//! Text to [`Value`]: granit-parser events composed into a tree whose scalars resolve
//! as gopkg.in/yaml.v3 v3.0.1 resolves them into `interface{}` (resolve.go `resolve`,
//! decode.go `scalar`).
//!
//! - A plain scalar without a tag (or with the non-specific `!`) resolves by yaml.v3's
//!   table: `true`/`True`/`TRUE` and the false forms, the null forms, `.inf`, `.nan`,
//!   integers in every `strconv.ParseInt(_, 0, 64)` spelling (underscores dropped, `0x`,
//!   `0o`, `0b`, a leading `0` for octal), then `ParseUint`, then YAML-style floats, then
//!   `0b`/`0o` with a signed rest. A timestamp (`2001-12-14`) is its RFC 3339 text tagged
//!   [`crate::config::TIMESTAMP_TAG`], standing in for Go's `time.Time`.
//! - A quoted or block scalar, and any scalar tagged `!!str`, is text.
//! - `!!bool`, `!!int`, `!!float`, `!!null` and `!!timestamp` resolve the text and fail
//!   when it is another kind (`!!float` takes an integer); `!!binary` is base64-decoded
//!   as Go's `base64.StdEncoding` (CR and LF skipped), and must be UTF-8 here.
//! - Any other tag (`!keep`, `!!seq` on a scalar, `%TAG` handles, verbatim tags) leaves
//!   the scalar as its text; tags on collections are dropped, as an untyped decode does.
//! - Mapping keys keep serde_yaml_ng's reading, which cpa-core's typed decoding was built
//!   on: plain YAML 1.2 core-schema numbers, booleans and nulls, and text otherwise
//!   (`1_000`, `007` and `2001-12-14` stay text, as yaml.v3 reads such a key into a
//!   string-keyed map). An integer key past 64 bits is a float when decimal and text
//!   otherwise, as yaml.v3 resolves it.
//! - `<<` stays a key; [`crate::config::expand_merges`] applies merges.
//! - Aliases are copied. Two keys with the same text are an error, as in yaml.v3, and so
//!   are two that resolve to the same key (yaml.v3 keeps the second; refused so a
//!   mapping keeps one entry per key event, which `config::binary` relies on).
//! - [`from_str`] reads the first document, as `yaml.Unmarshal` does; [`from_str_multi`]
//!   reads them all, as a `yaml.Decoder` does.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use granit_parser::{Event, Parser, ScalarStyle};

use super::{Error, Mapping, Tag, TaggedValue, Value, ng};

/// Nesting deeper than this, flow and block levels together, is refused.
/// ponytail: yaml.v3 allows 10 000 flow levels; [`Value`]'s recursive drop, equality
/// and serde impls cap it here. Raise it once those walk iteratively.
pub(crate) const MAX_DEPTH: usize = 512;
/// Nodes a document may hold once its aliases are copied out (yaml.v3 refuses
/// "excessive aliasing" by a ratio of alias copies to nodes).
/// ponytail: a fixed memory bound; yaml.v3's ratio would refuse earlier.
const MAX_NODES: usize = 1 << 24;

const TIMESTAMP_TAG: &str = crate::config::TIMESTAMP_TAG;

fn parser(text: &str) -> Parser<'_, granit_parser::StrInput<'_>> {
    let options = granit_parser::options! {
        flow_nesting_limit: MAX_DEPTH,
        block_nesting_limit: MAX_DEPTH,
        emit_comments: false,
    };
    Parser::new_from_str_with_options(text, options)
}

/// The first document of `text`, `Null` when there is none.
pub fn from_str(text: &str) -> Result<Value, Error> {
    match compose(text, true)?.first() {
        Some(node) => value(node, false),
        None => Ok(Value::Null),
    }
}

/// Every document of `text`.
pub fn from_str_multi(text: &str) -> Result<Vec<Value>, Error> {
    compose(text, false)?.iter().map(|node| value(node, false)).collect()
}

/// The first document of `text` decoded into `T` as yaml.v3 decodes into a Go value
/// ([`Typed`]).
pub fn from_str_typed<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, Error> {
    let empty = Node::empty();
    let docs = compose(text, true)?;
    T::deserialize(Typed(docs.first().map_or(&empty, |n| n.as_ref())))
}

/// `text`'s first document as a tree whose scalars are their source text (yaml.v3's
/// `Node.Value`: `0x1F` stays `0x1F`, `'31'` is `31`, `!!binary c2VjcmV0` is
/// `c2VjcmV0`), except that one resolving to null stays null; keys resolve as in
/// [`from_str`]. Aliases are copied, merges are not applied.
pub(crate) fn texts(text: &str) -> Result<Value, Error> {
    fn walk(node: &Node) -> Result<Value, Error> {
        Ok(match node {
            Node::Scalar(s) => match scalar(s, false)? {
                Value::Null => Value::Null,
                _ => Value::String(s.text.clone()),
            },
            Node::Sequence(items) => Value::Sequence(items.iter().map(|n| walk(n)).collect::<Result<_, _>>()?),
            Node::Mapping(entries) => {
                let mut map = Mapping::with_capacity(entries.len());
                for (k, v) in entries {
                    map.insert(value(k, true)?, walk(v)?);
                }
                Value::Mapping(map)
            }
        })
    }
    match compose(text, true)?.first() {
        Some(node) => walk(node),
        None => Ok(Value::Null),
    }
}

/// A scalar as the parser reports it.
#[derive(Clone)]
pub(crate) struct Scalar {
    pub text: String,
    pub plain: bool,
    /// Its tag, `%TAG` handles applied (`tag:yaml.org,2002:int`, `!keep`, `!`).
    pub tag: Option<String>,
}

/// A document node. An alias shares its anchor's node.
pub(crate) enum Node {
    Scalar(Scalar),
    Sequence(Vec<Rc<Node>>),
    Mapping(Vec<(Rc<Node>, Rc<Node>)>),
}

impl Node {
    fn empty() -> Self {
        Node::Scalar(Scalar {
            text: String::new(),
            plain: true,
            tag: None,
        })
    }
}

/// What a node costs once its aliases are copied out.
#[derive(Clone, Copy, Default)]
struct Size {
    /// Nodes, alias copies included.
    nodes: usize,
    /// Alias uses, those inside alias copies included (serde_yaml_ng's `jumpcount`).
    jumps: usize,
}

impl std::ops::AddAssign for Size {
    fn add_assign(&mut self, other: Size) {
        self.nodes = self.nodes.saturating_add(other.nodes);
        self.jumps = self.jumps.saturating_add(other.jumps);
    }
}

enum Frame {
    Sequence {
        items: Vec<Rc<Node>>,
        anchor: usize,
        size: Size,
    },
    Mapping {
        entries: Vec<(Rc<Node>, Rc<Node>)>,
        anchor: usize,
        size: Size,
        /// The key read, waiting for its value.
        key: Option<Rc<Node>>,
        /// The text of each scalar key so far.
        texts: HashSet<String>,
    },
}

impl Frame {
    fn expects_key(&self) -> bool {
        matches!(self, Frame::Mapping { key: None, .. })
    }
}

/// The documents of `text` (the first only, when `first_only`, as `yaml.Unmarshal`
/// reads it without parsing further).
pub(crate) fn compose(text: &str, first_only: bool) -> Result<Vec<Rc<Node>>, Error> {
    let text = nel_as_crlf(text);
    let mut docs = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    // Each anchor id's finished node and its size.
    let mut anchors: HashMap<usize, (Rc<Node>, Size)> = HashMap::new();
    let mut root: Option<(Rc<Node>, Size)> = None;
    let mut events = 0usize;
    for event in parser(&text) {
        let (event, _span) = event.map_err(|e| Error::msg(format!("yaml: {e}")))?;
        events += 1;
        // A finished node, its anchor and its size.
        let done: Option<(Rc<Node>, usize, Size)> = match event {
            Event::StreamStart | Event::StreamEnd => None,
            Event::DocumentStart(..) => {
                anchors.clear();
                root = None;
                None
            }
            Event::DocumentEnd => {
                let (node, size) = root.take().unwrap_or_else(|| (Rc::new(Node::empty()), Size::default()));
                // serde_yaml_ng's bound on alias uses, and a bound on the copies they make.
                if size.jumps > events.saturating_mul(100) || size.nodes > MAX_NODES {
                    return Err(Error::msg("yaml: document contains excessive aliasing"));
                }
                docs.push(node);
                if first_only {
                    break;
                }
                None
            }
            Event::Scalar(value, style, anchor, tag) => {
                let key = stack.last().is_some_and(Frame::expects_key);
                if key && let Some(Frame::Mapping { texts, .. }) = stack.last_mut() {
                    // yaml.v3 compares keys by kind and text.
                    if !texts.insert(value.to_string()) {
                        return Err(Error::msg(format!("yaml: mapping key {value:?} already defined")));
                    }
                }
                let scalar = Scalar {
                    text: value.into_owned(),
                    plain: style == ScalarStyle::Plain,
                    tag: tag.map(|t| format!("{}{}", t.handle(), t.suffix())),
                };
                Some((Rc::new(Node::Scalar(scalar)), anchor, Size { nodes: 1, jumps: 0 }))
            }
            Event::Alias(id) => {
                let (node, size) = anchors
                    .get(&id)
                    .ok_or_else(|| Error::msg("yaml: anchor value contains itself"))?;
                let mut size = *size;
                size.jumps = size.jumps.saturating_add(1);
                Some((node.clone(), 0, size))
            }
            Event::SequenceStart(_, anchor, _) => {
                stack.push(Frame::Sequence {
                    items: Vec::new(),
                    anchor,
                    size: Size { nodes: 1, jumps: 0 },
                });
                None
            }
            Event::MappingStart(_, anchor, _) => {
                stack.push(Frame::Mapping {
                    entries: Vec::new(),
                    anchor,
                    size: Size { nodes: 1, jumps: 0 },
                    key: None,
                    texts: HashSet::new(),
                });
                None
            }
            Event::SequenceEnd | Event::MappingEnd => match stack.pop() {
                Some(Frame::Sequence { items, anchor, size }) => Some((Rc::new(Node::Sequence(items)), anchor, size)),
                Some(Frame::Mapping {
                    entries, anchor, size, ..
                }) => Some((Rc::new(Node::Mapping(entries)), anchor, size)),
                None => return Err(Error::msg("yaml: unbalanced collection end")),
            },
            _ => None,
        };
        if stack.len() > MAX_DEPTH {
            return Err(Error::msg(format!("yaml: nesting exceeds {MAX_DEPTH} levels")));
        }
        let Some((node, anchor, size)) = done else {
            continue;
        };
        if anchor != 0 {
            anchors.insert(anchor, (node.clone(), size));
        }
        match stack.last_mut() {
            None => root = Some((node, size)),
            Some(Frame::Sequence { items, size: total, .. }) => {
                *total += size;
                items.push(node);
            }
            Some(Frame::Mapping {
                entries,
                key,
                size: total,
                ..
            }) => {
                *total += size;
                match key.take() {
                    None => *key = Some(node),
                    Some(k) => entries.push((k, node)),
                }
            }
        }
    }
    Ok(docs)
}

/// `text` with each NEL (U+0085) as CRLF, which is as long. libyaml, which yaml.v3 is
/// a port of, reads a NEL as a line break and as LF in content; granit-parser follows
/// YAML 1.2, where it is an ordinary character.
pub(crate) fn nel_as_crlf(text: &str) -> Cow<'_, str> {
    if text.contains('\u{85}') {
        Cow::Owned(text.replace('\u{85}', "\r\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// `node` resolved; `key` when it is a mapping key.
pub(crate) fn value(node: &Node, key: bool) -> Result<Value, Error> {
    Ok(match node {
        Node::Scalar(s) => scalar(s, key)?,
        Node::Sequence(items) => {
            Value::Sequence(items.iter().map(|item| value(item, false)).collect::<Result<_, _>>()?)
        }
        Node::Mapping(entries) => {
            let mut map = Mapping::with_capacity(entries.len());
            for (k, v) in entries {
                let k = value(k, true)?;
                if map.contains_key(&k) {
                    return Err(Error::msg(format!("yaml: duplicate mapping key {}", key_text(&k))));
                }
                let v = value(v, false)?;
                map.insert(k, v);
            }
            Value::Mapping(map)
        }
    })
}

fn key_text(key: &Value) -> String {
    match key {
        Value::String(s) => format!("{s:?}"),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        _ => "of a collection".into(),
    }
}

/// yaml.v3's short form of a tag (`tag:yaml.org,2002:int` is `!!int`); `None` for no
/// tag or the non-specific `!`, which resolves a plain scalar as untagged.
fn short_tag(s: &Scalar) -> Option<Cow<'_, str>> {
    let tag = s.tag.as_deref().filter(|t| *t != "!")?;
    Some(match tag.strip_prefix("tag:yaml.org,2002:") {
        Some(name) => Cow::Owned(format!("!!{name}")),
        None => Cow::Borrowed(tag),
    })
}

/// What a scalar reads as in an untyped value.
fn scalar(s: &Scalar, key: bool) -> Result<Value, Error> {
    let text = || Value::String(s.text.clone());
    let Some(tag) = short_tag(s) else {
        // A quoted scalar is text.
        if !s.plain {
            return Ok(text());
        }
        if key {
            return Ok(key_value(&s.text));
        }
        return Ok(resolve(&s.text, true).1);
    };
    match tag.as_ref() {
        "!!str" => Ok(text()),
        "!!binary" => {
            // Invalid base64, or bytes a Rust string cannot hold (Go reads them as a
            // string): kept as its base64 text, tagged, as serde_yaml_ng kept every
            // `!!binary`. The config load's binary pass refuses both with yaml.v3's
            // messages, so a file that is YAML stays a config error, not a parse error.
            let tagged = || {
                Value::Tagged(Box::new(TaggedValue {
                    tag: Tag::new("!!binary"),
                    value: text(),
                }))
            };
            Ok(match go_base64(s.text.as_bytes()).map(String::from_utf8) {
                Some(Ok(text)) => Value::String(text),
                Some(Err(_)) | None => tagged(),
            })
        }
        wanted @ ("!!bool" | "!!int" | "!!float" | "!!null" | "!!timestamp") => {
            let (kind, resolved) = resolve(&s.text, wanted == "!!timestamp");
            if kind == wanted {
                return Ok(resolved);
            }
            if wanted == "!!float" && kind == "!!int" {
                return Ok(resolved.as_f64().map_or(resolved, Value::from));
            }
            Err(Error::msg(format!(
                "yaml: cannot decode {kind} `{}` as a {wanted}",
                s.text
            )))
        }
        // Unknown to yaml.v3's resolver: the scalar is its text.
        _ => Ok(text()),
    }
}

/// A [`Node`] as a [`serde::Deserializer`] with yaml.v3's rules for decoding into a
/// typed Go value (decode.go `scalar`): a string field takes any scalar's text (a
/// `!!binary` one decoded), a bool field also takes the YAML 1.1 words (`yes`, `off`),
/// an integer field takes a float truncated, a float field an integer, and null is
/// the zero value of any field.
pub(crate) struct Typed<'a>(pub &'a Node);

impl Typed<'_> {
    fn scalar(&self) -> Option<&Scalar> {
        match self.0 {
            Node::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// The scalar's untyped value, `None` for a collection.
    fn resolved(&self) -> Result<Option<Value>, Error> {
        self.scalar().map(|s| scalar(s, false)).transpose()
    }

    fn is_null(&self) -> Result<bool, Error> {
        Ok(matches!(self.resolved()?, Some(Value::Null)))
    }

    fn invalid(&self, expected: &str) -> Error {
        let found = match self.0 {
            Node::Scalar(s) => format!("`{}`", s.text),
            Node::Sequence(_) => "a sequence".into(),
            Node::Mapping(_) => "a mapping".into(),
        };
        Error::msg(format!("yaml: cannot unmarshal {found} into {expected}"))
    }
}

struct TypedSeq<'a>(std::slice::Iter<'a, Rc<Node>>);

impl<'de> serde::de::SeqAccess<'de> for TypedSeq<'_> {
    type Error = Error;
    fn next_element_seed<S: serde::de::DeserializeSeed<'de>>(&mut self, seed: S) -> Result<Option<S::Value>, Error> {
        self.0.next().map(|n| seed.deserialize(Typed(n))).transpose()
    }
}

struct TypedMap<'a> {
    entries: std::slice::Iter<'a, (Rc<Node>, Rc<Node>)>,
    value: Option<&'a Node>,
}

impl<'de> serde::de::MapAccess<'de> for TypedMap<'_> {
    type Error = Error;
    fn next_key_seed<K: serde::de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        let Some((k, v)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some(v.as_ref());
        seed.deserialize(Typed(k)).map(Some)
    }
    fn next_value_seed<V: serde::de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let v = self
            .value
            .take()
            .ok_or_else(|| Error::msg("yaml: value without a key"))?;
        seed.deserialize(Typed(v))
    }
}

macro_rules! typed_int {
    ($($name:ident)*) => {$(
        fn $name<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.resolved()? {
                Some(Value::Null) => visitor.visit_u64(0),
                Some(Value::Number(n)) => {
                    if let Some(u) = n.as_u64() {
                        visitor.visit_u64(u)
                    } else if let Some(i) = n.as_i64() {
                        visitor.visit_i64(i)
                    } else {
                        let f = n.as_f64().unwrap_or(f64::NAN);
                        // Go converts a float in int64 range by truncation.
                        if (-9.223372036854776e18..9.223372036854776e18).contains(&f) {
                            visitor.visit_i64(f as i64)
                        } else {
                            Err(self.invalid("an integer"))
                        }
                    }
                }
                _ => Err(self.invalid("an integer")),
            }
        }
    )*};
}

impl<'de> serde::Deserializer<'de> for Typed<'_> {
    type Error = Error;

    fn deserialize_any<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Node::Scalar(s) => serde::Deserializer::deserialize_any(scalar(s, false)?, visitor),
            Node::Sequence(items) => visitor.visit_seq(TypedSeq(items.iter())),
            Node::Mapping(entries) => visitor.visit_map(TypedMap {
                entries: entries.iter(),
                value: None,
            }),
        }
    }

    fn deserialize_bool<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.resolved()? {
            Some(Value::Null) => visitor.visit_bool(false),
            Some(Value::Bool(b)) => visitor.visit_bool(b),
            Some(Value::String(s)) => match s.as_str() {
                "y" | "Y" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => visitor.visit_bool(true),
                "n" | "N" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => visitor.visit_bool(false),
                _ => Err(self.invalid("a bool")),
            },
            _ => Err(self.invalid("a bool")),
        }
    }

    typed_int! { deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
    deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128 }

    fn deserialize_f32<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.resolved()? {
            Some(Value::Null) => visitor.visit_f64(0.0),
            Some(Value::Number(n)) => visitor.visit_f64(n.as_f64().unwrap_or(f64::NAN)),
            _ => Err(self.invalid("a float")),
        }
    }

    fn deserialize_char<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let Some(s) = self.scalar() else {
            return Err(self.invalid("a string"));
        };
        match scalar(s, false)? {
            Value::Null => visitor.visit_str(""),
            // A `!!binary` scalar decodes; any other is its text.
            Value::String(decoded) if short_tag(s).as_deref() == Some("!!binary") => visitor.visit_string(decoded),
            _ => visitor.visit_str(&s.text),
        }
    }

    fn deserialize_string<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_byte_buf<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_option<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.is_null()? {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.is_null()? {
            visitor.visit_unit()
        } else {
            Err(self.invalid("null"))
        }
    }

    fn deserialize_unit_struct<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Node::Sequence(items) => visitor.visit_seq(TypedSeq(items.iter())),
            _ if self.is_null()? => visitor.visit_seq(TypedSeq([].iter())),
            _ => Err(self.invalid("a sequence")),
        }
    }

    fn deserialize_tuple<V: serde::de::Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        _: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Node::Mapping(entries) => visitor.visit_map(TypedMap {
                entries: entries.iter(),
                value: None,
            }),
            _ if self.is_null()? => visitor.visit_map(TypedMap {
                entries: [].iter(),
                value: None,
            }),
            _ => Err(self.invalid("a mapping")),
        }
    }

    fn deserialize_struct<V: serde::de::Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: serde::de::Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        serde::Deserializer::deserialize_enum(value(self.0, false)?, name, variants, visitor)
    }

    fn deserialize_identifier<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: serde::de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }
}

/// A plain mapping key as serde_yaml_ng read it, with yaml.v3's reading of an integer
/// past 64 bits.
fn key_value(text: &str) -> Value {
    match ng::plain(text) {
        ng::Plain::Null => Value::Null,
        ng::Plain::Bool(b) => Value::Bool(b),
        ng::Plain::U64(n) => Value::Number(n.into()),
        ng::Plain::I64(n) => Value::Number(n.into()),
        ng::Plain::F64(f) => Value::Number(f.into()),
        ng::Plain::Big => {
            let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
            let radix_prefix = unsigned.len() > 1 && unsigned.as_bytes()[1].is_ascii_alphabetic();
            match text.parse::<f64>() {
                Ok(f) if !radix_prefix => Value::Number(f.into()),
                _ => Value::String(text.to_owned()),
            }
        }
        ng::Plain::Str => Value::String(text.to_owned()),
    }
}

/// Go's `base64.StdEncoding` as yaml.v3 applies it: padding required, unused bits
/// ignored, CR and LF skipped. `None` for invalid data.
pub(crate) fn go_base64(text: &[u8]) -> Option<Vec<u8>> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    use base64::{Engine as _, alphabet};
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    );
    let payload: Vec<u8> = text.iter().copied().filter(|b| !matches!(b, b'\r' | b'\n')).collect();
    engine.decode(payload).ok()
}

/// yaml.v3 `resolve("", in)` (or with `!!timestamp`, which is the same here): the short
/// tag it resolves to and the value. Timestamps are tried only when `timestamps`.
pub(crate) fn resolve(input: &str, timestamps: bool) -> (&'static str, Value) {
    let text = || Value::String(input.to_owned());
    let hint = match input.as_bytes().first() {
        None => b'N',
        Some(b'+' | b'-') => b'S',
        Some(b'0'..=b'9') => b'D',
        Some(b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~') => b'M',
        Some(b'.') => b'.',
        Some(_) => return ("!!str", text()),
    };
    match input {
        "true" | "True" | "TRUE" => return ("!!bool", Value::Bool(true)),
        "false" | "False" | "FALSE" => return ("!!bool", Value::Bool(false)),
        "" | "~" | "null" | "Null" | "NULL" => return ("!!null", Value::Null),
        ".nan" | ".NaN" | ".NAN" => return ("!!float", Value::from(f64::NAN)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return ("!!float", Value::from(f64::INFINITY)),
        "-.inf" | "-.Inf" | "-.INF" => return ("!!float", Value::from(f64::NEG_INFINITY)),
        _ => {}
    }
    match hint {
        b'.' => {
            if let Some(f) = go_parse_float(input) {
                return ("!!float", Value::from(f));
            }
        }
        b'D' | b'S' => {
            if timestamps && let Some(time) = crate::config::timestamp::go_timestamp(input) {
                return (
                    "!!timestamp",
                    Value::Tagged(Box::new(TaggedValue {
                        tag: Tag::new(TIMESTAMP_TAG),
                        value: Value::String(time),
                    })),
                );
            }
            let plain = input.replace('_', "");
            if let Some(n) = go_parse_int(&plain, 0) {
                return ("!!int", Value::from(n));
            }
            if let Some(n) = go_parse_uint(&plain, 0) {
                return ("!!int", Value::from(n));
            }
            if yaml_style_float(&plain)
                && let Some(f) = go_parse_float(&plain)
            {
                return ("!!float", Value::from(f));
            }
            for (prefix, radix) in [("0b", 2), ("0o", 8)] {
                if let Some(rest) = plain.strip_prefix(prefix) {
                    if let Some(n) = go_parse_int(rest, radix) {
                        return ("!!int", Value::from(n));
                    }
                    if let Some(n) = go_parse_uint(rest, radix) {
                        return ("!!int", Value::from(n));
                    }
                } else if let Some(rest) = plain.strip_prefix(&format!("-{prefix}"))
                    && let Some(n) = go_parse_int(&format!("-{rest}"), radix)
                {
                    return ("!!int", Value::from(n));
                }
            }
        }
        _ => {}
    }
    ("!!str", text())
}

/// Go `strconv.ParseUint(s, base, 64)` for base 0 (prefixes `0b`, `0o`, `0x` in either
/// case, or a leading `0` for octal) or an explicit radix, without underscores.
fn go_parse_uint(s: &str, base: u32) -> Option<u64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let (radix, digits) = if base != 0 {
        (base, bytes)
    } else if bytes[0] == b'0' {
        match bytes.get(1).map(u8::to_ascii_lowercase) {
            Some(b'b') if bytes.len() >= 3 => (2, &bytes[2..]),
            Some(b'o') if bytes.len() >= 3 => (8, &bytes[2..]),
            Some(b'x') if bytes.len() >= 3 => (16, &bytes[2..]),
            _ => (8, &bytes[1..]),
        }
    } else {
        (10, bytes)
    };
    let mut n: u64 = 0;
    for &b in digits {
        let d = (b as char).to_digit(radix)?;
        n = n.checked_mul(u64::from(radix))?.checked_add(u64::from(d))?;
    }
    Some(n)
}

/// Go `strconv.ParseInt(s, base, 64)`: an optional sign, then [`go_parse_uint`].
fn go_parse_int(s: &str, base: u32) -> Option<i64> {
    let (negative, rest) = match s.as_bytes().first()? {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let n = go_parse_uint(rest, base)?;
    if negative {
        (n <= 1 << 63).then(|| (n as i64).wrapping_neg())
    } else {
        i64::try_from(n).ok()
    }
}

/// yaml.v3's `yamlStyleFloat`: `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn yaml_style_float(s: &str) -> bool {
    let digits = |s: &[u8]| s.iter().take_while(|b| b.is_ascii_digit()).count();
    let mut b = s.as_bytes();
    if let [b'+' | b'-', rest @ ..] = b {
        b = rest;
    }
    if let [b'.', rest @ ..] = b {
        let n = digits(rest);
        if n == 0 {
            return false;
        }
        b = &rest[n..];
    } else {
        let n = digits(b);
        if n == 0 {
            return false;
        }
        b = &b[n..];
        if let [b'.', rest @ ..] = b {
            b = &rest[digits(rest)..];
        }
    }
    if let [b'e' | b'E', rest @ ..] = b {
        let rest = match rest {
            [b'+' | b'-', r @ ..] => r,
            r => r,
        };
        let n = digits(rest);
        return n > 0 && n == rest.len();
    }
    b.is_empty()
}

/// Go `strconv.ParseFloat(s, 64)` for decimal text: underscores only between digits,
/// out-of-range values refused. Callers pass text that cannot spell `inf`, `nan` or a
/// hex float.
fn go_parse_float(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let unsigned = bytes
        .strip_prefix(b"+")
        .or_else(|| bytes.strip_prefix(b"-"))
        .unwrap_or(bytes);
    // `underscoreOK`: each underscore between digits.
    let mut saw_digit = false;
    let mut saw_underscore = false;
    for &b in unsigned {
        if b.is_ascii_digit() {
            saw_digit = true;
            saw_underscore = false;
        } else if b == b'_' {
            if !saw_digit {
                return None;
            }
            saw_digit = false;
            saw_underscore = true;
        } else {
            if saw_underscore {
                return None;
            }
            saw_digit = false;
        }
    }
    if saw_underscore {
        return None;
    }
    let cleaned: Cow<'_, str> = if s.contains('_') {
        Cow::Owned(s.replace('_', ""))
    } else {
        Cow::Borrowed(s)
    };
    // Rust's grammar would also take `inf` and `nan`, which callers never pass.
    if cleaned
        .bytes()
        .any(|b| b.is_ascii_alphabetic() && !matches!(b, b'e' | b'E'))
    {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|f| f.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn untyped(text: &str) -> Value {
        from_str(text).unwrap()
    }

    #[test]
    fn plain_scalars_resolve_as_yaml_v3() {
        let num = |text: &str| untyped(text);
        assert_eq!(num("1_000"), Value::from(1000));
        assert_eq!(num("0777"), Value::from(511));
        assert_eq!(num("08"), Value::from(8.0));
        assert_eq!(num("0o17"), Value::from(15));
        assert_eq!(num("0O17"), Value::from(15));
        assert_eq!(num("0X1f"), Value::from(31));
        assert_eq!(num("-0x1F"), Value::from(-31));
        assert_eq!(num("0b-1"), Value::from(-1));
        assert_eq!(num("+18446744073709551615"), Value::from(18446744073709551615.0));
        assert_eq!(num("18446744073709551615"), Value::from(u64::MAX));
        assert_eq!(num("1e400"), Value::from("1e400"));
        assert_eq!(num(".5_0"), Value::from(0.5));
        assert_eq!(num("1__0"), Value::from(10));
        assert_eq!(num("yes"), Value::from("yes"));
        assert_eq!(num("<<"), Value::from("<<"));
        assert_eq!(num("~"), Value::Null);
        assert_eq!(num("! 12"), Value::from(12));
        assert_eq!(num("'12'"), Value::from("12"));
        assert_eq!(num("!keep 0x1F"), Value::from("0x1F"));
        assert_eq!(num("!!float 1"), Value::from(1.0));
        assert_eq!(num("!!binary c2VjcmV0"), Value::from("secret"));
        assert!(from_str("!!int 1.5").is_err());
        assert!(from_str("!!bool yes").is_err());
        match num("2001-12-14") {
            Value::Tagged(t) => {
                assert_eq!(t.tag, TIMESTAMP_TAG);
                assert_eq!(t.value, Value::from("2001-12-14T00:00:00Z"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keys_keep_their_serde_yaml_ng_reading() {
        let map = untyped("1_000: a\n007: b\n2001-12-14: c\n1: d\n18446744073709551616: e\n0x10000000000000000: f\n");
        let keys: Vec<Value> = map.as_mapping().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                Value::from("1_000"),
                Value::from("007"),
                Value::from("2001-12-14"),
                Value::from(1),
                Value::from(18446744073709551616.0),
                Value::from("0x10000000000000000"),
            ]
        );
    }

    #[test]
    fn documents_aliases_and_duplicates() {
        assert_eq!(untyped("a: 1\n---\nb: 2\n"), untyped("a: 1"));
        assert_eq!(from_str_multi("a: 1\n---\nb: 2\n").unwrap().len(), 2);
        assert_eq!(untyped(""), Value::Null);
        assert_eq!(untyped("a: &x [1]\nb: *x\n")["b"], untyped("[1]"));
        assert_eq!(untyped("a: &x 1\nb: &x 2\nc: *x\n")["c"], Value::from(2));
        assert!(from_str("a: 1\na: 2\n").is_err());
        assert!(from_str("a: 1\n'a': 2\n").is_err());
        assert!(from_str("1: a\n0x1: b\n").is_err());
        assert_eq!(untyped("\u{feff}a: 1\nb: 2\n")["b"], Value::from(2));
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert!(from_str(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(from_str(&ok).is_ok());
    }
}
