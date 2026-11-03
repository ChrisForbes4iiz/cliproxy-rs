//! [`Value`] to text, as serde_yaml_ng 0.10 wrote it through libyaml's emitter
//! (unsafe-libyaml 0.2.11 `emitter.rs`, with unicode on and no line width): block
//! collections indented by two, sequences in a mapping not indented, empty collections
//! in flow style, and each scalar in the first style libyaml allows of the one
//! serde_yaml_ng asks for. A string asks for a literal block when it holds a line
//! break, single quotes when serde_yaml_ng would read it plain as something else
//! (`'1'`, `'true'`, `'007'`, `''`), and plain otherwise. Config files that
//! [`crate::config::ConfigDocument`] rewrites keep their text because of this.

use super::value::tagged::nobang;
use super::{Error, Mapping, Number, Value, ng};

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Any,
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
}

/// libyaml's `yaml_emitter_analyze_scalar`.
struct Analysis {
    multiline: bool,
    flow_plain_allowed: bool,
    block_plain_allowed: bool,
    single_quoted_allowed: bool,
    block_allowed: bool,
}

fn is_printable(c: char) -> bool {
    matches!(c, '\n' | ' '..='~' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..) && c != '\u{feff}'
}

fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn is_blankz(c: Option<char>) -> bool {
    c.is_none_or(|c| c == ' ' || c == '\t' || is_break(c))
}

fn analyze(value: &str) -> Analysis {
    if value.is_empty() {
        return Analysis {
            multiline: false,
            flow_plain_allowed: false,
            block_plain_allowed: true,
            single_quoted_allowed: true,
            block_allowed: false,
        };
    }
    let mut block_indicators = false;
    let mut flow_indicators = false;
    let mut line_breaks = false;
    let mut special_characters = false;
    let mut leading_space = false;
    let mut leading_break = false;
    let mut trailing_space = false;
    let mut trailing_break = false;
    let mut break_space = false;
    let mut space_break = false;
    let mut previous_space = false;
    let mut previous_break = false;
    if value.starts_with("---") || value.starts_with("...") {
        block_indicators = true;
        flow_indicators = true;
    }
    let chars: Vec<char> = value.chars().collect();
    let mut preceded_by_whitespace = true;
    for (i, &c) in chars.iter().enumerate() {
        let followed_by_whitespace = is_blankz(chars.get(i + 1).copied());
        let last = i + 1 == chars.len();
        if i == 0 {
            if matches!(
                c,
                '#' | ',' | '[' | ']' | '{' | '}' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`'
            ) {
                flow_indicators = true;
                block_indicators = true;
            }
            if matches!(c, '?' | ':') {
                flow_indicators = true;
                if followed_by_whitespace {
                    block_indicators = true;
                }
            }
            if c == '-' && followed_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        } else {
            if matches!(c, ',' | '?' | '[' | ']' | '{' | '}') {
                flow_indicators = true;
            }
            if c == ':' {
                flow_indicators = true;
                if followed_by_whitespace {
                    block_indicators = true;
                }
            }
            if c == '#' && preceded_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        }
        if !is_printable(c) {
            special_characters = true;
        }
        if is_break(c) {
            line_breaks = true;
        }
        if c == ' ' {
            if i == 0 {
                leading_space = true;
            }
            if last {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(c) {
            if i == 0 {
                leading_break = true;
            }
            if last {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }
        preceded_by_whitespace = is_blankz(Some(c));
    }
    let mut a = Analysis {
        multiline: line_breaks,
        flow_plain_allowed: true,
        block_plain_allowed: true,
        single_quoted_allowed: true,
        block_allowed: true,
    };
    if leading_space || leading_break || trailing_space || trailing_break {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if trailing_space {
        a.block_allowed = false;
    }
    if break_space {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
    }
    if space_break || special_characters {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
        a.single_quoted_allowed = false;
        a.block_allowed = false;
    }
    if line_breaks {
        a.flow_plain_allowed = false;
        a.block_plain_allowed = false;
    }
    if flow_indicators {
        a.flow_plain_allowed = false;
    }
    if block_indicators {
        a.block_plain_allowed = false;
    }
    a
}

/// The style serde_yaml_ng's serializer asks for a string in (`serialize_str`).
fn string_style(value: &str) -> Style {
    if value.contains('\n') {
        return Style::Literal;
    }
    match ng::plain(value) {
        ng::Plain::Str if ng::digits_but_not_number(value) => Style::SingleQuoted,
        ng::Plain::Str => Style::Any,
        _ => Style::SingleQuoted,
    }
}

fn number_text(n: &Number) -> String {
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    if f.is_nan() {
        ".nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { ".inf" } else { "-.inf" }.into()
    } else {
        ryu::Buffer::new().format_finite(f).to_owned()
    }
}

/// A scalar node: its text, the style asked for, and its tag as written (`!name`).
struct Scalar<'a> {
    text: std::borrow::Cow<'a, str>,
    style: Style,
}

enum Node<'a> {
    Scalar(Scalar<'a>),
    Sequence(&'a [Value]),
    Mapping(&'a Mapping),
}

/// A node with the tag serde_yaml_ng writes for a [`Value::Tagged`]: `!` and the tag
/// without its leading `!`, which libyaml writes as the `!` handle and an escaped suffix.
fn node_of(value: &Value) -> Result<(Option<String>, Node<'_>), Error> {
    let (tag, value) = match value {
        Value::Tagged(tagged) => {
            if matches!(tagged.value, Value::Tagged(_)) {
                return Err(Error::msg("serializing nested enums in YAML is not supported yet"));
            }
            (Some(format!("!{}", nobang(&tagged.tag.string))), &tagged.value)
        }
        v => (None, v),
    };
    let plain = |t: &'static str| {
        Node::Scalar(Scalar {
            text: t.into(),
            style: Style::Plain,
        })
    };
    let node = match value {
        Value::Null => plain("null"),
        Value::Bool(true) => plain("true"),
        Value::Bool(false) => plain("false"),
        Value::Number(n) => Node::Scalar(Scalar {
            text: number_text(n).into(),
            style: Style::Plain,
        }),
        Value::String(s) => Node::Scalar(Scalar {
            text: s.as_str().into(),
            style: string_style(s),
        }),
        Value::Sequence(items) => Node::Sequence(items),
        Value::Mapping(m) => Node::Mapping(m),
        Value::Tagged(_) => unreachable!("untagged above"),
    };
    Ok((tag, node))
}

/// libyaml's `yaml_emitter_write_tag_content`: URI characters as they are, others
/// percent-encoded byte by byte.
fn tag_content(suffix: &str, out: &mut String) {
    for c in suffix.chars() {
        let uri = c.is_ascii_alphanumeric()
            || matches!(
                c,
                '_' | '-'
                    | ';'
                    | '/'
                    | '?'
                    | ':'
                    | '@'
                    | '&'
                    | '='
                    | '+'
                    | '$'
                    | ','
                    | '.'
                    | '~'
                    | '*'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
            );
        if uri {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
}

/// serde_yaml_ng's serializer fails on a one-entry mapping whose key is itself written
/// as a one-entry mapping (a tagged value is one): it takes such a key for a tag and
/// libyaml reports "expected DOCUMENT-START or STREAM-END".
fn refuse_mapping_key(map: &Mapping) -> Result<(), Error> {
    let one_entry_key = |k: &Value| match k {
        Value::Tagged(_) => true,
        Value::Mapping(m) => m.len() == 1,
        _ => false,
    };
    if map.len() == 1 && map.keys().next().is_some_and(one_entry_key) {
        return Err(Error::msg("emitter error: expected DOCUMENT-START or STREAM-END"));
    }
    Ok(())
}

struct Emitter {
    out: String,
    column: usize,
    whitespace: bool,
    indention: bool,
    indent: i32,
    indents: Vec<i32>,
    flow_level: usize,
    simple_key_context: bool,
    mapping_context: bool,
}

const BEST_INDENT: i32 = 2;

impl Emitter {
    fn put(&mut self, c: char) {
        self.out.push(c);
        self.column += 1;
    }

    fn put_break(&mut self) {
        self.out.push('\n');
        self.column = 0;
    }

    fn write(&mut self, s: &str) {
        self.out.push_str(s);
        self.column += s.chars().count();
    }

    fn increase_indent(&mut self, flow: bool, indentless: bool) {
        self.indents.push(self.indent);
        if self.indent < 0 {
            self.indent = if flow { BEST_INDENT } else { 0 };
        } else if !indentless {
            self.indent += BEST_INDENT;
        }
    }

    fn pop_indent(&mut self) {
        self.indent = self.indents.pop().unwrap_or(-1);
    }

    fn write_indent(&mut self) {
        let indent = self.indent.max(0) as usize;
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            self.put_break();
        }
        while self.column < indent {
            self.put(' ');
        }
        self.whitespace = true;
        self.indention = true;
    }

    fn write_indicator(&mut self, indicator: &str, need_whitespace: bool, is_whitespace: bool, is_indention: bool) {
        if need_whitespace && !self.whitespace {
            self.put(' ');
        }
        self.write(indicator);
        self.whitespace = is_whitespace;
        self.indention = self.indention && is_indention;
    }

    fn write_tag(&mut self, tag: &str) {
        // Handle `!`, then the suffix.
        if !self.whitespace {
            self.put(' ');
        }
        self.write("!");
        let mut suffix = String::new();
        tag_content(&tag[1..], &mut suffix);
        self.write(&suffix);
        self.whitespace = false;
        self.indention = false;
    }

    /// libyaml's `yaml_emitter_check_simple_key` for a mapping key.
    fn simple_key(tag: &Option<String>, node: &Node<'_>) -> bool {
        let tag_len = tag.as_ref().map_or(0, |t| t.len());
        let length = match node {
            Node::Scalar(s) => {
                if analyze(&s.text).multiline {
                    return false;
                }
                tag_len + s.text.len()
            }
            Node::Sequence([]) => tag_len,
            Node::Mapping(m) if m.is_empty() => tag_len,
            _ => return false,
        };
        length <= 128
    }

    fn emit_node(&mut self, value: &Value, mapping: bool, simple_key: bool) -> Result<(), Error> {
        let (tag, node) = node_of(value)?;
        self.mapping_context = mapping;
        self.simple_key_context = simple_key;
        match node {
            Node::Scalar(scalar) => self.emit_scalar(tag.as_deref(), &scalar),
            Node::Sequence(items) => {
                if let Some(tag) = &tag {
                    self.write_tag(tag);
                }
                if self.flow_level > 0 || items.is_empty() {
                    self.write_indicator("[", true, true, false);
                    self.increase_indent(true, false);
                    self.flow_level += 1;
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            self.write_indicator(",", false, false, false);
                        }
                        self.emit_node(item, false, false)?;
                    }
                    self.flow_level -= 1;
                    self.pop_indent();
                    self.write_indicator("]", false, false, false);
                } else {
                    let indentless = self.mapping_context && !self.indention;
                    self.increase_indent(false, indentless);
                    for item in items {
                        self.write_indent();
                        self.write_indicator("-", true, false, true);
                        self.emit_node(item, false, false)?;
                    }
                    self.pop_indent();
                }
                Ok(())
            }
            Node::Mapping(map) => {
                // Under a tag, serde_yaml_ng expects no second tag and writes it.
                if tag.is_none() {
                    refuse_mapping_key(map)?;
                }
                if let Some(tag) = &tag {
                    self.write_tag(tag);
                }
                if self.flow_level > 0 || map.is_empty() {
                    self.write_indicator("{", true, true, false);
                    self.increase_indent(true, false);
                    self.flow_level += 1;
                    for (i, (k, v)) in map.iter().enumerate() {
                        if i > 0 {
                            self.write_indicator(",", false, false, false);
                        }
                        let (key_tag, key_node) = node_of(k)?;
                        if Self::simple_key(&key_tag, &key_node) {
                            self.emit_node(k, true, true)?;
                            self.write_indicator(":", false, false, false);
                        } else {
                            self.write_indicator("?", true, false, false);
                            self.emit_node(k, true, false)?;
                            self.write_indicator(":", true, false, false);
                        }
                        self.emit_node(v, true, false)?;
                    }
                    self.flow_level -= 1;
                    self.pop_indent();
                    self.write_indicator("}", false, false, false);
                } else {
                    self.increase_indent(false, false);
                    for (k, v) in map {
                        self.write_indent();
                        let (key_tag, key_node) = node_of(k)?;
                        if Self::simple_key(&key_tag, &key_node) {
                            self.emit_node(k, true, true)?;
                            self.write_indicator(":", false, false, false);
                        } else {
                            self.write_indicator("?", true, false, true);
                            self.emit_node(k, true, false)?;
                            self.write_indent();
                            self.write_indicator(":", true, false, true);
                        }
                        self.emit_node(v, true, false)?;
                    }
                    self.pop_indent();
                }
                Ok(())
            }
        }
    }

    fn emit_scalar(&mut self, tag: Option<&str>, scalar: &Scalar<'_>) -> Result<(), Error> {
        let text = scalar.text.as_ref();
        let a = analyze(text);
        let mut style = scalar.style;
        if style == Style::Any {
            style = Style::Plain;
        }
        if self.simple_key_context && a.multiline {
            style = Style::DoubleQuoted;
        }
        if style == Style::Plain {
            if (self.flow_level > 0 && !a.flow_plain_allowed) || (self.flow_level == 0 && !a.block_plain_allowed) {
                style = Style::SingleQuoted;
            }
            if text.is_empty() && (self.flow_level > 0 || self.simple_key_context) {
                style = Style::SingleQuoted;
            }
        }
        if style == Style::SingleQuoted && !a.single_quoted_allowed {
            style = Style::DoubleQuoted;
        }
        if style == Style::Literal && (!a.block_allowed || self.flow_level > 0 || self.simple_key_context) {
            style = Style::DoubleQuoted;
        }
        if let Some(tag) = tag {
            self.write_tag(tag);
        }
        self.increase_indent(true, false);
        match style {
            Style::Plain | Style::Any => self.write_plain(text),
            Style::SingleQuoted => self.write_single_quoted(text),
            Style::DoubleQuoted => self.write_double_quoted(text),
            Style::Literal => self.write_literal(text),
        }
        self.pop_indent();
        Ok(())
    }

    fn write_plain(&mut self, text: &str) {
        if !self.whitespace && (!text.is_empty() || self.flow_level > 0) {
            self.put(' ');
        }
        // Plain text holds no line breaks (libyaml allows plain only without them) and
        // is never folded, the width being unlimited.
        self.write(text);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_single_quoted(&mut self, text: &str) {
        self.write_indicator("'", true, false, false);
        let mut breaks = false;
        for c in text.chars() {
            if is_break(c) {
                if !breaks && c == '\n' {
                    self.put_break();
                }
                if c == '\n' {
                    self.put_break();
                } else {
                    self.out.push(c);
                    self.column = 0;
                }
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                if c == '\'' {
                    self.put('\'');
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
        if breaks {
            self.write_indent();
        }
        self.write_indicator("'", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_double_quoted(&mut self, text: &str) {
        self.write_indicator("\"", true, false, false);
        for c in text.chars() {
            if !is_printable(c) || c == '\u{feff}' || is_break(c) || c == '"' || c == '\\' {
                self.put('\\');
                let code = c as u32;
                let short = match code {
                    0x00 => Some('0'),
                    0x07 => Some('a'),
                    0x08 => Some('b'),
                    0x09 => Some('t'),
                    0x0A => Some('n'),
                    0x0B => Some('v'),
                    0x0C => Some('f'),
                    0x0D => Some('r'),
                    0x1B => Some('e'),
                    0x22 => Some('"'),
                    0x5C => Some('\\'),
                    0x85 => Some('N'),
                    0xA0 => Some('_'),
                    0x2028 => Some('L'),
                    0x2029 => Some('P'),
                    _ => None,
                };
                match short {
                    Some(s) => self.put(s),
                    None if code <= 0xFF => self.write(&format!("x{code:02X}")),
                    None if code <= 0xFFFF => self.write(&format!("u{code:04X}")),
                    None => self.write(&format!("U{code:08X}")),
                }
            } else {
                self.put(c);
            }
        }
        self.write_indicator("\"", false, false, false);
        self.whitespace = false;
        self.indention = false;
    }

    fn write_literal(&mut self, text: &str) {
        self.write_indicator("|", true, false, false);
        // libyaml's `yaml_emitter_write_block_scalar_hints`.
        if text.starts_with(' ') || text.starts_with(is_break) {
            self.write_indicator(&BEST_INDENT.to_string(), false, false, false);
        }
        let mut rev = text.chars().rev();
        let chomp = match (rev.next(), rev.next()) {
            (None, _) => Some("-"),
            (Some(last), _) if !is_break(last) => Some("-"),
            (Some(_), None) => Some("+"),
            (Some(_), Some(before)) if is_break(before) => Some("+"),
            _ => None,
        };
        if let Some(chomp) = chomp {
            self.write_indicator(chomp, false, false, false);
        }
        self.put_break();
        self.indention = true;
        self.whitespace = true;
        let mut breaks = true;
        for c in text.chars() {
            if is_break(c) {
                if c == '\n' {
                    self.put_break();
                } else {
                    self.out.push(c);
                    self.column = 0;
                }
                self.indention = true;
                breaks = true;
            } else {
                if breaks {
                    self.write_indent();
                }
                self.put(c);
                self.indention = false;
                breaks = false;
            }
        }
    }
}

/// `value` as one YAML document, as serde_yaml_ng's `to_string` wrote it.
pub fn to_string(value: &Value) -> Result<String, Error> {
    let mut e = Emitter {
        out: String::new(),
        column: 0,
        whitespace: true,
        indention: true,
        indent: -1,
        indents: Vec::new(),
        flow_level: 0,
        simple_key_context: false,
        mapping_context: false,
    };
    e.emit_node(value, false, false)?;
    // Document end, implicit. serde_yaml_ng's `to_string` never ends the stream, so a
    // `|+` block at the end gets no `...`.
    e.write_indent();
    Ok(e.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::from_str;

    #[test]
    fn writes_what_serde_yaml_ng_wrote() {
        // Texts recorded from serde_yaml_ng 0.10.0 `to_string` on the same values.
        let cases: &[(&str, &str)] = &[
            ("a: 1", "a: 1\n"),
            ("{}", "{}\n"),
            ("[]", "[]\n"),
            ("a: []\nb: {}\n", "a: []\nb: {}\n"),
            ("a: [x, y]\n", "a:\n- x\n- y\n"),
            ("- a: 1\n  b: 2\n- - x\n  - y\n", "- a: 1\n  b: 2\n- - x\n  - y\n"),
            ("a: {b: {c: d}}", "a:\n  b:\n    c: d\n"),
            ("a: '1'", "a: '1'\n"),
            ("a: '007'", "a: '007'\n"),
            ("a: 'true'", "a: 'true'\n"),
            ("a: ''", "a: ''\n"),
            ("a: '~'", "a: '~'\n"),
            ("a: 'yes'", "a: yes\n"),
            ("a: 'x: y'", "a: 'x: y'\n"),
            ("a: ' x'", "a: ' x'\n"),
            ("a: '#x'", "a: '#x'\n"),
            ("a: \"\\t\"", "a: \"\\t\"\n"),
            ("a: \"l1\\nl2\\n\"", "a: |\n  l1\n  l2\n"),
            ("a: \"l1\\nl2\"", "a: |-\n  l1\n  l2\n"),
            ("a: \" l1\\nl2\"", "a: |2-\n   l1\n  l2\n"),
            ("\"k\\nk\": v", "? |-\n  k\n  k\n: v\n"),
            ("a: \"x\\n\\n\"", "a: |+\n  x\n\n"),
            (
                "a: 0.5\nb: 1.0\nc: .inf\nd: -.inf\n",
                "a: 0.5\nb: 1.0\nc: .inf\nd: -.inf\n",
            ),
            ("a: null\nb: ~\n", "a: null\nb: null\n"),
            ("a: é", "a: é\n"),
            ("'': x", "'': x\n"),
            ("? [a]\n: x\n", "? - a\n: x\n"),
        ];
        for (yaml, want) in cases {
            let value = from_str(yaml).unwrap();
            assert_eq!(to_string(&value).unwrap(), *want, "{yaml}");
        }
        let tagged = Value::Tagged(Box::new(crate::yaml::TaggedValue {
            tag: crate::yaml::Tag::new("!cpa-timestamp"),
            value: Value::from("2001-12-14T00:00:00Z"),
        }));
        let mut map = Mapping::new();
        map.insert("t".into(), tagged);
        assert_eq!(
            to_string(&Value::Mapping(map)).unwrap(),
            "t: !cpa-timestamp 2001-12-14T00:00:00Z\n"
        );
    }
}
