//! Tagged scalars as yaml.v3 reads them, where serde_yaml_ng reads them differently.
//! yaml.v3 base64-decodes a scalar whose tag resolves to `!!binary` into any string
//! field (decode.go `scalar`); serde_yaml_ng keeps the base64 text. yaml.v3 reads a
//! scalar with an unknown local tag (`!keep x`) as its text, resolves a plain scalar
//! with the non-specific `!` tag implicitly, and treats the literal short tags
//! `!!binary`, `!!null` and `!!str` that its `shortTag` keeps (`!<!!null>`,
//! `!%21binary`) as the standard ones; serde_yaml_ng wraps all of those as tagged
//! values. The tags come from granit-parser's events, so handles, `%TAG` directives and
//! `%XX` escapes resolve as they do for yaml.v3.
//!
//! What this does not follow is refused rather than read differently: other literal
//! short tags, a non-specific tag that resolves to a number or boolean, tagged or
//! aliased mapping keys, collection keys (Go rejects them too), a repeated anchor name (serde_yaml_ng binds a redefined
//! anchor's aliases to a different node than libyaml does), a `%00` escape in a
//! tag (libyaml cuts the tag there; Go keeps the NUL), and a quoted `<<` key (yaml.v3's
//! `isMerge` reads it as a plain key; [`super::expand_merges`] cannot tell it apart).
//! A merge operand that is an alias of a sequence fails with yaml.v3's error, which
//! serde_yaml_ng's expanded aliases no longer show.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::yaml::Value;
use anyhow::{Context, bail, ensure};

const UNSUPPORTED_TAG: &str = "a YAML tag in the config file is not supported";
const UNSUPPORTED_KEY: &str = "a tagged or aliased mapping key in the config file is not supported";
const COLLECTION_KEY: &str = "a mapping or sequence used as a mapping key in the config file is not supported";
const QUOTED_MERGE_KEY: &str = "a quoted << mapping key in the config file is not supported";
const REPEATED_ANCHOR: &str = "a YAML anchor name is defined more than once in the config file";
const PLUGIN_PIN: &str =
    "a plugin's store.version or store.release-tag in the config file is a !!binary value, which is not supported";
/// More alias copies of rewritten scalars than a config has; alias chains past it are
/// refused. Each scalar written in the file counts once and is not limited.
const MAX_COPIES: usize = 1 << 20;
/// More entries than aliases of a config visit, kept or not: copies an alias drops (a
/// number that lands outside a string setting) still cost a step each.
const MAX_VISITS: usize = 1 << 22;

/// Go's `base64.StdEncoding` as yaml.v3 applies it: padding required, unused bits
/// ignored, CR and LF skipped. `None` for invalid data.
pub(super) fn go_base64(text: &[u8]) -> Option<Vec<u8>> {
    crate::yaml::go_base64(text)
}

/// Rewrites the tagged scalars in `value`, which serde_yaml_ng parsed from `text` with
/// merges not yet expanded, to what yaml.v3 reads, aliases of them included. Fails on
/// invalid base64 as Go does, and refuses what it does not follow (see the module
/// docs) and binary bytes that are not UTF-8, which a setting cannot hold.
/// With `literals`, a plain scalar in a [`literal_field`] is its text, as Go's typed
/// decode reads it into the config; `text` is the value at those keys of the config
/// (empty for a whole file). See [`Mode`] for what else `mode` changes.
pub(super) fn decode(text: &str, value: &mut Value, literals: Option<&[&str]>, mode: Mode) -> anyhow::Result<()> {
    // Without a tag, an anchor, a `<<` key or a collection key, serde_yaml_ng reads the
    // file as yaml.v3 does here.
    if mode != Mode::Plugins
        && !text.contains(['!', '&'])
        && !needs_walk(value)
        && !literals.is_some_and(|at| scalar_literal_field(value, &mut at.iter().map(|k| Some(*k)).collect()))
    {
        return Ok(());
    }
    let mut walk = Walk {
        events: events(text)?.into_iter(),
        defined: HashSet::new(),
        sequences: HashSet::new(),
        anchors: HashMap::new(),
        anchored: 0,
        last_field: None,
        literals: literals.map(|at| at.iter().map(|k| Some(Rc::from(*k))).collect()),
        keys: Vec::new(),
        found: Vec::new(),
        copies: 0,
        visits: 0,
        mode,
    };
    if let Some(root) = walk.events.next() {
        walk.node(root, &mut Vec::new(), false)?;
    }
    // Go reads a plugin's pin from the raw node (pluginhost `yamlScalarString`), so a
    // `!!binary` pin, aliased or merged in too, keeps its base64 text there and fails
    // the version grammar; refused rather than read decoded.
    ensure!(
        !walk.found.iter().any(|f| f.kind == Kind::Binary && plugin_pin(&f.keys)),
        PLUGIN_PIN
    );
    // yaml.v3 reads any plain scalar into a string field as its text; serde_yaml_ng
    // reads a number or boolean (`1.50`, `0x1F`, `007`, `true`) as that value, whose text
    // differs. The fields of [`literal_field`] take the text.
    let mut found: Vec<Found> = walk
        .found
        .into_iter()
        .filter(|f| f.kind != Kind::Literal(false))
        .collect();
    // Paths are entry or item positions; patch them in one ordered pass.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    apply(value, 0, &found)
}

/// Whether `value` holds a `<<` key or a mapping or sequence used as a key (whose own
/// keys serde_yaml_ng's merge expansion never visits).
fn needs_walk(value: &Value) -> bool {
    match value {
        Value::Mapping(m) => m.iter().any(|(k, v)| {
            matches!(k, Value::Mapping(_) | Value::Sequence(_)) || k.as_str() == Some("<<") || needs_walk(v)
        }),
        Value::Sequence(items) => items.iter().any(needs_walk),
        Value::Tagged(tagged) => needs_walk(&tagged.value),
        _ => false,
    }
}

fn apply(node: &mut Value, depth: usize, patches: &[Found]) -> anyhow::Result<()> {
    let Some(first) = patches.first() else {
        return Ok(());
    };
    if first.path.len() == depth {
        *node = first.value.clone();
        return Ok(());
    }
    let mut node = node;
    while let Value::Tagged(tagged) = node {
        node = &mut tagged.value;
    }
    let children: Box<dyn Iterator<Item = &mut Value>> = match node {
        Value::Mapping(m) => Box::new(m.values_mut()),
        Value::Sequence(items) => Box::new(items.iter_mut()),
        _ => bail!("config YAML changed while it was read"),
    };
    let mut rest = patches;
    for (i, child) in children.enumerate() {
        if rest.is_empty() {
            break;
        }
        let n = rest.iter().take_while(|f| f.path[depth] == i).count();
        apply(child, depth + 1, &rest[..n])?;
        rest = &rest[n..];
    }
    ensure!(rest.is_empty(), "config YAML changed while it was read");
    Ok(())
}

/// Whether `value`, with no merge keys or aliases, holds a number or boolean in a
/// [`literal_field`]. `keys` leads to `value`.
fn scalar_literal_field<'a>(value: &'a Value, keys: &mut Vec<Option<&'a str>>) -> bool {
    match value {
        Value::Mapping(m) => m.iter().any(|(k, v)| {
            // A key that is not a string still names a mapping entry.
            keys.push(Some(k.as_str().unwrap_or("")));
            let found = scalar_literal_field(v, keys);
            keys.pop();
            found
        }),
        Value::Sequence(items) => items.iter().any(|v| {
            keys.push(None);
            let found = scalar_literal_field(v, keys);
            keys.pop();
            found
        }),
        // A plain timestamp is tagged ([`super::timestamp::TAG`]).
        Value::Number(_) | Value::Bool(_) | Value::Tagged(_) => literal_field(keys),
        _ => false,
    }
}

/// `value` as [`decode`] with literals reads it back once written: a number or boolean
/// in a [`literal_field`] is its text. For a document a JSON body edited.
pub(super) fn as_reread(value: &mut Value) {
    fn walk(value: &mut Value, keys: &mut Vec<Option<String>>) {
        match value {
            Value::Mapping(m) => {
                for (k, v) in m.iter_mut() {
                    keys.push(Some(k.as_str().unwrap_or("").to_owned()));
                    walk(v, keys);
                    keys.pop();
                }
            }
            Value::Sequence(items) => {
                for v in items {
                    keys.push(None);
                    walk(v, keys);
                    keys.pop();
                }
            }
            Value::Number(_) | Value::Bool(_) if literal_field(keys) => *value = Value::String(super::go_string(value)),
            _ => {}
        }
    }
    walk(value, &mut Vec::new());
}

/// `keys` (a mapping key, or `None` for a list item, per step) without merge keys and
/// the list items of a `<<: [...]`.
fn named<K: AsRef<str>>(keys: &[Option<K>]) -> Vec<Option<&str>> {
    let mut named = Vec::with_capacity(keys.len());
    let mut keys = keys.iter().map(|k| k.as_ref().map(AsRef::as_ref)).peekable();
    while let Some(key) = keys.next() {
        if key == Some("<<") {
            // `<<: [*a, *b]`: the list item is part of the merge too.
            keys.next_if(Option::is_none);
        } else {
            named.push(key);
        }
    }
    named
}

/// Whether the scalar at `keys` is a Go string field (in the v8 layout or the legacy one
/// it migrates from), which yaml.v3 reads any plain scalar into as its text. Untyped
/// data keeps its YAML types: plugin configs, and a payload rule's `params` and
/// `match`/`not-match` values, which reach requests with them.
fn literal_field<K: AsRef<str>>(keys: &[Option<K>]) -> bool {
    super::schema::is_string(&super::document::v8_path(&named(keys)))
}

/// Whether `keys` lead into a `plugins.configs` entry.
fn plugin_config<K: AsRef<str>>(keys: &[Option<K>]) -> bool {
    matches!(named(keys).as_slice(), [Some("plugins"), Some("configs"), Some(_), ..])
}

/// Whether `keys` lead to a plugin's `store.version` or `store.release-tag`.
fn plugin_pin<K: AsRef<str>>(keys: &[Option<K>]) -> bool {
    matches!(
        named(keys).as_slice(),
        [
            Some("plugins"),
            Some("configs"),
            Some(_),
            Some("store"),
            Some("version" | "release-tag")
        ]
    )
}

/// The document's node events, in order, without stream and document markers.
enum Event {
    Scalar {
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        plain: bool,
        value: Vec<u8>,
    },
    Alias(Vec<u8>),
    Start {
        anchor: Option<Vec<u8>>,
        mapping: bool,
    },
    End,
}

/// Fails on a tag holding a NUL (a `%00` escape), which libyaml cut and Go keeps;
/// refused as it was while these events came from libyaml.
fn events(text: &str) -> anyhow::Result<Vec<Event>> {
    events_with(text, None)
}

/// An untagged plain scalar of a text.
pub(super) struct Plain {
    /// Its byte span in the text.
    pub span: std::ops::Range<usize>,
    pub value: Vec<u8>,
    /// Where its [`super::timestamp::TAG`] starts, when it has that tag.
    pub timestamp_tag: Option<usize>,
}

/// Each plain scalar of `text` that is untagged or has [`super::timestamp::TAG`].
pub(super) fn plain_scalars(text: &str) -> anyhow::Result<Vec<Plain>> {
    let mut out = Vec::new();
    events_with(text, Some(&mut out))?;
    Ok(out)
}

/// [`events`], recording each untagged plain scalar in `plain`. Only the first
/// document is read, as [`crate::yaml::from_str`] reads it.
fn events_with(text: &str, mut plain: Option<&mut Vec<Plain>>) -> anyhow::Result<Vec<Event>> {
    use granit_parser::Event as E;
    let options = granit_parser::options! {
        emit_anchor_events: true,
        emit_comments: false,
        flow_nesting_limit: crate::yaml::load::MAX_DEPTH,
        block_nesting_limit: crate::yaml::load::MAX_DEPTH,
    };
    // granit-parser keeps a `%00` escape in a tag as a NUL, as Go does; refused still,
    // as it was while the tags came from libyaml, which cut them there.
    let nul = |tag: &Option<Vec<u8>>| tag.as_ref().is_some_and(|t| t.contains(&0));
    let full = |tag: Option<std::borrow::Cow<'_, granit_parser::Tag>>| {
        tag.map(|t| format!("{}{}", t.handle(), t.suffix()).into_bytes())
    };
    let mut out = Vec::new();
    // Each anchor id's name.
    let mut names: HashMap<usize, Vec<u8>> = HashMap::new();
    let text = crate::yaml::load::nel_as_crlf(text);
    for item in granit_parser::Parser::new_from_str_with_options(&text, options) {
        // from_str parsed the same text, so this does not fail.
        let (event, span) = item.map_err(|e| anyhow::anyhow!("failed to parse config file: {e}"))?;
        let next = match event {
            E::DocumentEnd => return Ok(out),
            E::Anchor(id, name) => {
                let name = name.into_owned().into_bytes();
                names.insert(id, name.clone());
                // It follows the node it names.
                match out.last_mut() {
                    Some(Event::Scalar { anchor, .. } | Event::Start { anchor, .. }) => *anchor = Some(name),
                    _ => bail!("config YAML is malformed"),
                }
                None
            }
            E::Scalar(value, style, _, tag) => {
                let tag = full(tag);
                ensure!(!nul(&tag), UNSUPPORTED_TAG);
                let is_plain = style == granit_parser::ScalarStyle::Plain;
                let timestamp = tag.as_deref() == Some(super::timestamp::TAG.as_bytes());
                let value = value.into_owned().into_bytes();
                let source = span.start.byte_offset().zip(span.end.byte_offset());
                if let Some(plain) = plain.as_deref_mut()
                    && is_plain
                    && (tag.is_none() || timestamp)
                    && let Some((start, end)) = source
                {
                    plain.push(Plain {
                        span: start..end,
                        value: value.clone(),
                        timestamp_tag: if timestamp {
                            span.tag_start.and_then(|m| m.byte_offset())
                        } else {
                            None
                        },
                    });
                }
                Some(Event::Scalar {
                    anchor: None,
                    tag,
                    plain: is_plain,
                    value,
                })
            }
            E::Alias(id) => Some(Event::Alias(
                names.get(&id).cloned().context("config YAML is malformed")?,
            )),
            E::SequenceStart(_, _, tag) => {
                ensure!(!nul(&full(tag)), UNSUPPORTED_TAG);
                Some(Event::Start {
                    anchor: None,
                    mapping: false,
                })
            }
            E::MappingStart(_, _, tag) => {
                ensure!(!nul(&full(tag)), UNSUPPORTED_TAG);
                Some(Event::Start {
                    anchor: None,
                    mapping: true,
                })
            }
            E::SequenceEnd | E::MappingEnd => Some(Event::End),
            _ => None,
        };
        out.extend(next);
    }
    Ok(out)
}

/// yaml.v3's null spellings.
fn is_null(value: &[u8]) -> bool {
    matches!(value, b"" | b"~" | b"null" | b"Null" | b"NULL")
}

/// A plain scalar that yaml.v3 resolves to a string, judged without resolving it: text
/// that starts with a letter and is not a boolean word. Numbers, timestamps, `.inf`,
/// `<<` and the like start with something else.
fn resolves_to_text(value: &[u8]) -> bool {
    const WORDS: &[&[u8]] = &[b"true", b"false", b"yes", b"no", b"on", b"off", b"y", b"n"];
    value.first().is_some_and(u8::is_ascii_alphabetic) && !WORDS.iter().any(|w| value.eq_ignore_ascii_case(w))
}

/// What yaml.v3 reads for a tagged scalar, where serde_yaml_ng reads something else;
/// `None` where they agree.
fn rewrite(tag: &[u8], plain: bool, value: Vec<u8>) -> anyhow::Result<Option<Value>> {
    let text = || {
        String::from_utf8(value.clone())
            .ok()
            .context("config YAML is not UTF-8")
    };
    Ok(Some(match tag {
        b"tag:yaml.org,2002:binary" | b"!!binary" => {
            let bytes = go_base64(&value).context("yaml: !!binary value contains invalid base64 data")?;
            let text = String::from_utf8(bytes)
                .ok()
                .context("a !!binary value in the config file is not UTF-8 text")?;
            Value::String(text)
        }
        // Non-specific: a plain scalar resolves as if untagged, a quoted one is text.
        b"!" if plain && is_null(&value) => Value::Null,
        b"!" if plain && !resolves_to_text(&value) => bail!(UNSUPPORTED_TAG),
        b"!" | b"!!str" => Value::String(text()?),
        b"!!null" if is_null(&value) => Value::Null,
        t if t.starts_with(b"!!") => bail!(UNSUPPORTED_TAG),
        // Any other local tag is unknown to yaml.v3: the scalar is its text.
        t if t.starts_with(b"!") => Value::String(text()?),
        _ => return Ok(None),
    }))
}

/// A rewritten scalar.
#[derive(Clone)]
struct Found {
    /// Entry or item positions.
    path: Vec<usize>,
    /// The mapping key, or `None` for a list item, at each position.
    keys: Vec<Key>,
    /// What yaml.v3 reads.
    value: Value,
    kind: Kind,
}

type Key = Option<Rc<str>>;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Tag,
    Binary,
    /// The text of a plain scalar serde_yaml_ng may read as a number or boolean, or of
    /// one tagged `!!int`, `!!float` or `!!bool`; applied only where it is a
    /// [`literal_field`] (`true`), and kept elsewhere only for an anchor's aliases.
    Literal(bool),
}

struct Walk {
    events: std::vec::IntoIter<Event>,
    defined: HashSet<Vec<u8>>,
    /// Anchors of sequences.
    sequences: HashSet<Vec<u8>>,
    /// Each anchor's rewritten scalars, by their path below it.
    anchors: HashMap<Vec<u8>, Vec<Found>>,
    /// How many anchored collections are around the current node.
    anchored: usize,
    /// The keys [`Self::literal`] last judged, and whether they are a [`literal_field`].
    last_field: Option<(Vec<Key>, bool)>,
    /// Whether literals are read at all, and the config keys the text sits at.
    literals: Option<Vec<Key>>,
    /// The key of each position in the current path.
    keys: Vec<Key>,
    /// Rewritten scalars, by their path from the root.
    found: Vec<Found>,
    /// How many of them are alias copies.
    copies: usize,
    /// How many anchor entries aliases visited, [`MAX_VISITS`] at most.
    visits: usize,
    mode: Mode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// A config file: literals are its string settings.
    File,
    /// A JSON management body, whose `<<` keys are merges: Go saves its keys plain.
    Body,
    /// Literals are the scalars of plugin configs, as Go hands a plugin its raw node,
    /// tagged [`PLAIN`]: `007` is a string to serde_yaml_ng too, but quoted it is another.
    Plugins,
}

/// The tag of a plain scalar's text in [`Mode::Plugins`].
pub const PLAIN: &str = "!cpa-plain";

/// `text` as a YAML double-quoted scalar that reads back as `text`: line breaks, C0 and
/// C1 controls, DEL, U+2028, U+2029, U+FEFF, U+FFFE and U+FFFF escaped.
pub fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            '\u{feff}' => out.push_str("\\uFEFF"),
            '\u{fffe}' => out.push_str("\\uFFFE"),
            '\u{ffff}' => out.push_str("\\uFFFF"),
            c if c < ' ' || ('\u{7f}'..='\u{9f}').contains(&c) => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Walk {
    fn next(&mut self) -> anyhow::Result<Event> {
        self.events.next().context("config YAML ended early")
    }

    fn define(&mut self, anchor: &Option<Vec<u8>>) -> anyhow::Result<()> {
        if let Some(name) = anchor {
            ensure!(self.defined.insert(name.clone()), REPEATED_ANCHOR);
        }
        Ok(())
    }

    /// A literal at `keys`, if it is kept: it is a [`literal_field`], or it is anchored,
    /// so an alias may make it one. Decided here, not over the tree afterwards, so the
    /// cost stays linear in the file and the aliases' copies.
    /// A literal's value: its text, or in [`Mode::Plugins`] the YAML that writes it as
    /// Go writes the raw node, tagged [`PLAIN`]: a standard tag stays (`!!float 1.50`),
    /// and a quoted scalar it tags stays quoted.
    fn text_value(&self, text: String, tag: Option<&[u8]>, plain: bool) -> Value {
        if self.mode != Mode::Plugins {
            return Value::String(text);
        }
        let text = match tag.and_then(|t| t.strip_prefix(b"tag:yaml.org,2002:")) {
            Some(name) => {
                let name = String::from_utf8_lossy(name);
                let text = if plain { text } else { double_quoted(&text) };
                format!("!!{name} {text}")
            }
            None => text,
        };
        Value::Tagged(Box::new(crate::yaml::TaggedValue {
            tag: crate::yaml::Tag::new(PLAIN),
            value: Value::String(text),
        }))
    }

    fn literal(&mut self, keys: &[Key], anchored: bool) -> Option<Kind> {
        let at = self.literals.as_ref()?;
        // The items of one list share their keys.
        let field = match &self.last_field {
            Some((last, field)) if last.as_slice() == keys => *field,
            _ => {
                let field = if self.mode == Mode::Plugins {
                    plugin_config(keys)
                } else if at.is_empty() {
                    literal_field(keys)
                } else {
                    literal_field(&[at.as_slice(), keys].concat())
                };
                self.last_field = Some((keys.to_vec(), field));
                field
            }
        };
        (field || anchored).then_some(Kind::Literal(field))
    }

    fn node(&mut self, event: Event, path: &mut Vec<usize>, key: bool) -> anyhow::Result<()> {
        let start = self.found.len();
        let anchor = match event {
            Event::Scalar {
                anchor,
                tag,
                plain,
                value,
            } => {
                self.define(&anchor)?;
                let anchored = anchor.is_some() || self.anchored > 0;
                let literal = match &tag {
                    // yaml.v3 reads a scalar tagged as a number or boolean into a string
                    // field as its text, as it does an untagged one. A plugin also gets
                    // a timestamp with its tag, which `any` reads as a time.
                    Some(tag) => {
                        matches!(
                            tag.as_slice(),
                            b"tag:yaml.org,2002:int" | b"tag:yaml.org,2002:float" | b"tag:yaml.org,2002:bool"
                        ) || (self.mode == Mode::Plugins && tag.as_slice() == b"tag:yaml.org,2002:timestamp")
                    }
                    None => plain && !is_null(&value) && !resolves_to_text(&value),
                };
                // yaml.v3's layout code matches keys by their node, tag included
                // (`!!str <<` is no merge key).
                ensure!(tag.is_none() || !key, UNSUPPORTED_KEY);
                if literal {
                    if key {
                        // An anchored key: its aliases are values, which take its text.
                        if let Some(name) = &anchor {
                            let text = String::from_utf8(value).ok().context("config YAML is not UTF-8")?;
                            let found = Found {
                                path: Vec::new(),
                                keys: Vec::new(),
                                value: self.text_value(text, None, true),
                                kind: Kind::Literal(false),
                            };
                            self.anchors.insert(name.clone(), vec![found]);
                            return Ok(());
                        }
                    } else if let Some(kind) = {
                        let keys = std::mem::take(&mut self.keys);
                        let kind = self.literal(&keys, anchored);
                        self.keys = keys;
                        kind
                    } {
                        let text = String::from_utf8(value).ok().context("config YAML is not UTF-8")?;
                        let value = self.text_value(text, tag.as_deref(), plain);
                        self.found.push(Found {
                            path: path.clone(),
                            keys: self.keys.clone(),
                            value,
                            kind,
                        });
                    }
                } else if let Some(tag) = tag {
                    let kind = match tag.as_slice() {
                        b"tag:yaml.org,2002:binary" | b"!!binary" => Kind::Binary,
                        _ => Kind::Tag,
                    };
                    if let Some(v) = rewrite(&tag, plain, value)? {
                        self.found.push(Found {
                            path: path.clone(),
                            keys: self.keys.clone(),
                            value: v,
                            kind,
                        });
                    }
                }
                anchor
            }
            Event::Alias(name) => {
                ensure!(!key, UNSUPPORTED_KEY);
                self.visits += self.anchors.get(&name).map_or(0, Vec::len);
                ensure!(self.visits <= MAX_VISITS, "config YAML aliases expand too far");
                let copies = self.anchors.get(&name).cloned().unwrap_or_default();
                for mut below in copies {
                    let keys = [self.keys.as_slice(), &below.keys].concat();
                    if let Kind::Literal(_) = below.kind {
                        match self.literal(&keys, self.anchored > 0) {
                            Some(kind) => below.kind = kind,
                            None => continue,
                        }
                    }
                    self.copies += 1;
                    ensure!(self.copies <= MAX_COPIES, "config YAML aliases expand too far");
                    self.found.push(Found {
                        path: [path.as_slice(), &below.path].concat(),
                        keys,
                        ..below
                    });
                }
                None
            }
            Event::Start { anchor, mapping } => {
                // Paths address mapping values only; Go rejects collection keys.
                ensure!(!key, COLLECTION_KEY);
                self.define(&anchor)?;
                if let Some(name) = anchor.as_ref().filter(|_| !mapping) {
                    self.sequences.insert(name.clone());
                }
                if anchor.is_some() {
                    self.anchored += 1;
                }
                let mut i = 0;
                loop {
                    let event = self.next()?;
                    if matches!(event, Event::End) {
                        break;
                    }
                    if mapping {
                        // yaml.v3 `isMerge` takes an untagged `<<` key only when plain; a
                        // quoted one is refused here, a tagged one below.
                        let merge = matches!(&event, Event::Scalar { tag: None, value, .. } if value == b"<<");
                        ensure!(
                            !merge || self.mode == Mode::Body || matches!(event, Event::Scalar { plain: true, .. }),
                            QUOTED_MERGE_KEY
                        );
                        let name: Rc<str> = match &event {
                            Event::Scalar { value, .. } => String::from_utf8_lossy(value).into(),
                            _ => "".into(),
                        };
                        self.node(event, path, true)?;
                        let value = self.next()?;
                        // yaml.v3 `merge`: an alias operand must name a mapping.
                        if merge && let Event::Alias(name) = &value {
                            ensure!(!self.sequences.contains(name), super::MERGE_NOT_MAPS);
                        }
                        path.push(i);
                        self.keys.push(Some(name));
                        self.node(value, path, false)?;
                    } else {
                        path.push(i);
                        self.keys.push(None);
                        self.node(event, path, false)?;
                    }
                    path.pop();
                    self.keys.pop();
                    i += 1;
                }
                if anchor.is_some() {
                    self.anchored -= 1;
                }
                anchor
            }
            Event::End => bail!("config YAML is malformed"),
        };
        if let Some(name) = anchor {
            let below = self.found[start..]
                .iter()
                .map(|f| Found {
                    path: f.path[path.len()..].to_vec(),
                    keys: f.keys[path.len()..].to_vec(),
                    ..f.clone()
                })
                .collect();
            self.anchors.insert(name, below);
        }
        Ok(())
    }
}
