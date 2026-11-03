"""Writes the yaml.v3 parity corpus as exact bytes (LF unless a case says otherwise)."""
import os
import sys

out = sys.argv[1]
os.makedirs(out, exist_ok=True)

C = {}

C["bool_yaml11_words"] = """a: yes
b: no
c: on
d: off
e: y
f: n
g: Yes
h: NO
i: On
j: OFF
"""
C["bool_case_forms"] = """a: true
b: True
c: TRUE
d: false
e: False
f: FALSE
g: tRUE
h: fAlse
"""
C["int_forms"] = """dec: 123
neg: -17
plus: +17
zero: 0
negzero: -0
oct_yaml11: 017
oct_yaml12: 0o17
oct_upper: 0O17
oct_bad: 0o19
leading_zero: 007
eight: 08
nine: 09
hex: 0x1F
hex_upper: 0X1f
neg_hex: -0x1F
bin: 0b101
neg_bin: -0b101
under: 1_000
under_hex: 0x_1F
under_lead: _1
under_double: 1__0
"""
C["int_ranges"] = """max_i64: 9223372036854775807
min_i64: -9223372036854775808
over_i64: 9223372036854775808
max_u64: 18446744073709551615
over_u64: 18446744073709551616
under_i64: -9223372036854775809
huge_hex: 0x10000000000000000
"""
C["float_forms"] = """f1: 1.5
f2: -1.5e3
f3: 1e3
f4: .5
f5: 1.
f6: 6.8523015e+5
f7: 685.230_15e+03
f8: 1_000.5
f9: 0.1
f10: 1.0
f11: -0.0
f12: 1E3
f13: +1.5
exp_only: 1e
dot_only: .
sexagesimal: 190:20:30.15
sexagesimal_int: 190:20:30
"""
C["float_specials"] = """inf: .inf
Inf: .Inf
INF: .INF
pinf: +.inf
ninf: -.inf
nan: .nan
NaN: .NaN
NAN: .NAN
iNf: .iNf
bare_inf: inf
"""
C["null_forms"] = """a: ~
b: null
c: Null
d: NULL
e:
f: nULL
g: ""
h: ''
"""
C["null_key"] = """~: tilde-key
b: c
"""
C["null_root"] = "~\n"
C["empty_document"] = ""
C["only_comment"] = "# nothing here\n"
C["only_doc_start"] = "---\n"
C["scalar_root"] = "hello world\n"
C["sequence_root"] = "- 1\n- two\n- 3.0\n"
C["timestamps"] = """date: 2001-12-14
canonical: 2001-12-15T02:59:43.1Z
iso_lower_t: 2001-12-14t21:59:43.10-05:00
spaced: 2001-12-14 21:59:43.10 -5
no_zone: 2001-12-14 21:59:43.10
quoted: "2001-12-14"
bad_month: 2001-13-14
short_year: 01-12-14
"""
C["timestamp_tagged"] = """t: !!timestamp 2001-12-14
"""
C["anchors_aliases"] = """base: &b
  x: 1
  y: [1, 2]
ref: *b
list:
  - &s str
  - *s
num: &n 42
num_ref: *n
"""
C["alias_undefined"] = "a: *nope\n"
C["anchor_redefined"] = """a: &x 1
b: &x 2
c: *x
"""
C["merge_single"] = """base: &b {x: 1, y: 2}
m:
  <<: *b
  y: 9
"""
C["merge_list"] = """b1: &b1 {x: 1, y: 2}
b2: &b2 {y: 3, z: 4}
m:
  <<: [*b1, *b2]
"""
C["merge_after_key"] = """base: &b {x: 1, y: 2}
m:
  y: 0
  <<: *b
"""
C["merge_inline"] = "m: {<<: {a: 1}, b: 2}\n"
C["merge_scalar_error"] = "m:\n  <<: 1\n"
C["merge_nested_chain"] = """a: &a {x: 1}
b: &b
  <<: *a
  y: 2
c:
  <<: *b
  z: 3
"""
C["merge_quoted_key"] = """m:
  "<<": {a: 1}
"""
C["tag_str"] = "a: !!str 123\nb: !!str true\nc: !!str ~\n"
C["tag_int_quoted"] = 'a: !!int "42"\n'
C["tag_int_bad"] = "a: !!int abc\n"
C["tag_float_from_int"] = "a: !!float 1\n"
C["tag_bool_quoted"] = 'a: !!bool "true"\n'
C["tag_bool_yes"] = "a: !!bool yes\n"
C["tag_null"] = 'a: !!null ""\n'
C["tag_binary"] = "a: !!binary aGVsbG8=\n"
C["tag_binary_bad"] = "a: !!binary '%%%'\n"
C["tag_custom_scalar"] = "a: !foo bar\n"
C["tag_custom_mapping"] = "a: !foo {b: 1}\n"
C["tag_nonspecific"] = "a: ! 12\nb: ! text\n"
C["tag_verbatim"] = "a: !<tag:yaml.org,2002:str> 5\n"
C["tag_seq_map"] = "s: !!seq [1]\nm: !!map {a: 1}\n"
C["tag_set"] = "s: !!set {a, b}\n"
C["tag_omap"] = "o: !!omap [a: 1, b: 2]\n"
C["tag_directive"] = "%TAG !e! tag:example.com,2000:\n---\na: !e!foo bar\n"
C["yaml_directive_11"] = "%YAML 1.1\n---\na: yes\nb: 017\n"
C["dup_keys"] = "a: 1\na: 2\n"
C["dup_keys_nested"] = "x:\n  a: 1\n  b: 2\n  a: 3\n"
C["dup_keys_flow"] = "{a: 1, a: 2}\n"
C["dup_keys_null_spellings"] = "~: 1\nnull: 2\n"
C["dup_keys_quoted_vs_plain"] = "a: 1\n'a': 2\n"
C["dup_keys_int_forms"] = "1: a\n0x1: b\n"
C["multi_doc"] = "a: 1\n---\nb: 2\n"
C["multi_doc_markers"] = "---\na: 1\n...\n---\nb: 2\n...\n"
C["multi_doc_empty_first"] = "---\n---\na: 1\n"
C["flow_styles"] = "{a: [1, 2, {b: c}], d: {e: f}, g: [], h: {}}\n"
C["flow_multiline"] = "a: [\n  1,\n  2,\n]\nb: {\n  c: d,\n}\n"
C["block_sequences"] = """a:
- 1
- - 2
  - 3
- k: v
  k2: v2
b:
  - x
"""
C["block_scalars"] = """lit: |
  line1
  line2
lit_strip: |-
  x
lit_keep: |+
  x

fold: >
  a
  b

  c
fold_strip: >-
  a
  b
fold_keep: >+
  a

indent_indicator: |2
    two
more_indented: >
  a
    b
  c
last: end
"""
C["quoting_escapes"] = r"""sq: 'it''s'
dq: "tab\tnl\nuni\u00e9 \x41 \U0001F600"
dq_nul: "a\0b"
dq_bell: "\a\b\e\f\v"
dq_nel: "\N\_\L\P"
dq_line_cont: "a\
  b"
plain_colon: a:b
plain_hash: a#b
plain_comment: a # c
plain_multi: a
  b
dq_multi: "a
  b"
sq_multi: 'a

  b'
empty_sq: ''
leading_space: '  x'
"""
C["comments_everywhere"] = """# header
a: 1 # trailing
# between
b:
  # inside
  c: 2 # nested trailing
d: [1, # in flow
  2]
e: "#not a comment"
f: 'x' #comment
"""
C["bom"] = "\ufeffa: 1\nb: 2\n"
C["bom_doc_start"] = "\ufeff---\na: 1\n"
C["tabs_in_value"] = "a: x\ty\nb:\tz\n"
C["tabs_indent_error"] = "a:\n\tb: 1\n"
C["tab_after_seq"] = "-\t1\n"
C["crlf_lines"] = "a: 1\r\nb: |\r\n  x\r\n  y\r\nc: >\r\n  p\r\n  q\r\n"
C["cr_only"] = "a: 1\rb: 2\r"
C["deep_flow_64"] = "a: " + "[" * 64 + "1" + "]" * 64 + "\n"
C["deep_flow_200"] = "a: " + "[" * 200 + "1" + "]" * 200 + "\n"
C["deep_block_150"] = "".join("  " * i + "k%d:\n" % i for i in range(150)) + "  " * 150 + "v\n"
C["key_types"] = "1: int\n2.5: float\ntrue: bool\n0x10: hex\nstr: s\n"
C["key_all_ints"] = "1: a\n2: b\n"
C["key_complex"] = "? [a, b]\n: c\n"
C["key_mapping"] = "? {a: 1}\n: c\n"
C["key_long_explicit"] = "? a\n: 1\n? b\n"
C["unicode"] = "名前: 値\nemoji: \U0001F600\n"
C["nel_in_plain"] = "a: x\u0085b\n"
C["quoting_slash_escape"] = r'a: "a\/b"' + "\n"
C["plain_dash_value"] = "d: -\ne: +\n"
C["special_plain_strings"] = """a: 0x
b: 0o
c: 0b
f: .5.
g: 1.2.3
h: v1.10
i: 1:2
j: '010'
k: 12e
l: 0x1G
"""
C["indentation_odd"] = "a:\n   b: 1\n   c:\n         - x\n         - y\n"
C["invalid_unclosed_flow"] = "a: [1, 2\n"
C["invalid_bad_indent"] = "a: 1\n b: 2\n"
C["invalid_mapping_in_scalar"] = "a: b: c\n"
C["long_string_1k"] = "a: " + "x" * 1024 + "\n"
C["many_aliases"] = "a: &a [1, 2, 3]\nb: [*a, *a, *a, *a, *a, *a, *a, *a]\nc: [*a, *a]\n"
C["config_like"] = """host: ""
port: 8317
debug: false
api-keys:
  - "k1"
  - k2
proxy-url: socks5://user:pass@127.0.0.1:1080
request-retry: 3
ratio: 0.5
headers:
  X-Test: yes
  X-Num: 017
nested:
  name: n
  enabled: on
"""
# Typed decode cases: Go decodes into the recorder's `shape` struct (Go's config
# reads typed fields the same way); every case is also decoded into `shape`.
C["typed_bool_yes"] = "debug: yes\n"
C["typed_bool_on"] = "debug: on\n"
C["typed_bool_y"] = "debug: y\n"
C["typed_bool_True"] = "debug: True\n"
C["typed_bool_quoted"] = 'debug: "true"\n'
C["typed_bool_int"] = "debug: 1\n"
C["typed_int_quoted"] = 'port: "8317"\n'
C["typed_int_float"] = "port: 8317.0\n"
C["typed_int_octal12"] = "port: 0o17\n"
C["typed_int_octal11"] = "port: 017\n"
C["typed_int_under"] = "port: 8_317\n"
C["typed_int_hex"] = "port: 0x20\n"
C["typed_int_overflow"] = "port: 99999999999999999999\n"
C["typed_int_bool"] = "port: true\n"
C["typed_int_null"] = "port: ~\n"
C["typed_string_int"] = "host: 123\n"
C["typed_string_bool"] = "host: true\n"
C["typed_string_float"] = "host: 1.50\n"
C["typed_string_null"] = "host: ~\n"
C["typed_string_timestamp"] = "host: 2001-12-14\n"
C["typed_string_hex"] = "host: 0x1F\n"
C["typed_string_seq"] = "host: [a]\n"
C["typed_seq_scalar"] = "api-keys: abc\n"
C["typed_seq_null"] = "api-keys: ~\n"
C["typed_seq_mixed"] = "api-keys: [a, 1, true, ~]\n"
C["typed_float_int"] = "ratio: 1\n"
C["typed_float_inf"] = "ratio: .inf\n"
C["typed_float_string"] = "ratio: abc\n"
C["typed_map_int_key"] = "headers: {1: a}\n"
C["typed_map_null_value"] = "headers: {a: ~}\n"
C["typed_unknown_field"] = "foo: bar\nport: 1\n"
C["typed_root_sequence"] = "- 1\n"
C["typed_root_scalar"] = "8317\n"

for name, text in C.items():
    with open(os.path.join(out, name + ".yaml"), "wb") as f:
        f.write(text.encode("utf-8"))
print(len(C), "cases written to", out)
