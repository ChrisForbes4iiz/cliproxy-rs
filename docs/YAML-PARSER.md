# YAML parser choice

cliproxy-rs reads `config.yaml` (a trust boundary for the v8 management API), plugin
manifests and a few provider files with serde_yaml_ng 0.10, whose parser is
unsafe-libyaml 0.2.11 (RUSTSEC-2024-0449, unmaintained), and cpa-core also calls
unsafe-libyaml directly for parser events. This page records which safe-Rust library
replaces both (bead cliproxy-rs-7ea.44.1, spike of cliproxy-rs-7ea.44), the evidence,
and the migration plan for cliproxy-rs-7ea.44.2 and .44.3.

## Decision

**Use the serde-saphyr family: `granit-parser` 2.1 for events and `serde-saphyr` 2.1
for serde (de)serialization, behind a cpa-core-owned `cpa_core::yaml` module with
its own ordered `Value`.** Build that `Value` from granit-parser events with a port
of yaml.v3's `resolve.go`, so scalar resolution follows Go rather than any library's
defaults, and use serde-saphyr where a crate deserializes a typed struct straight
from text and for `to_string`.

What decided it:

1. **Parity.** On the 119-case corpus, serde-saphyr with Go-leaning options
   (`strict_booleans`, `legacy_octal_numbers`, non-finite floats passed through,
   depth budget 10 000) differs from yaml.v3 in 16 cases, against 35 for
   serde_yaml_ng, 34 for noyalib and 36 for saphyr. On the 199 config-shaped inputs
   that cpa-core's other Go fixtures already replay, it differs in 25, against 105,
   100 and 107. Most of that gap is behaviour cpa-core ports by hand today:
   serde-saphyr base64-decodes `!!binary`, expands `<<` merge keys, reads `0o17`,
   `0X1f`, `1_000` and `08` the way Go does, reads a custom-tagged scalar
   (`!keep x`) as its text, rejects duplicate keys, and skips a leading BOM.
2. **Events with what binary.rs needs.** granit-parser is the only candidate that
   exposes the events cpa-core reads from libyaml today: resolved tags (`%TAG`
   handles and `%XX` escapes applied, and `%00` kept as a NUL, as Go keeps it, where
   libyaml cuts the tag), scalar style, anchor ids plus anchor-name events
   (`emit_anchor_events`), alias events, and byte offsets that already skip a BOM.
   noyalib's parser module is private. saphyr-parser reports character indices and
   anchor ids only.
3. **Safety and maintenance.** granit-parser and serde-saphyr both declare
   `#![forbid(unsafe_code)]`. They are released together, the latest on 2026-11-02
   and 2026-11-03, and serde-saphyr has 242 GitHub stars and 15.6M downloads.
   noyalib is version 0.0.58, with 11 stars and one maintainer, and its 52 000 lines
   are mostly features cliproxy-rs would not use (CST, SIMD, schema, tokio).
4. **MSRV.** serde-saphyr needs 1.89 and granit-parser 1.81. The workspace minimum is
   1.99.

What it costs: serde-saphyr has no `Value` type, so cpa-core owns one (it needs one
anyway, to keep other crates off the library). Its emitter double-quotes strings and
also quotes YAML 1.1 words (`"n"`, `"inf"` as a key) where serde_yaml_ng
single-quotes, so `yaml_string` output changes and must be pinned (see the migration
plan). A minimal from_str and to_string binary is about 1 MB larger than one built on
serde_yaml_ng (annotate-snippets and encoding_rs are not optional).

## What the code needs

Counted in `crates/` at 3e3bf29a (lines naming `serde_yaml_ng`, `unsafe_libyaml` or
`yaml_edit`):

| crate | serde_yaml_ng | unsafe_libyaml | yaml_edit |
|---|---|---|---|
| cpa-server | 127 | 0 | 0 |
| cpa-core | 85 | 29 | 16 |
| cpa-exec | 25 | 0 | 0 |
| cpa-plugin | 10 | 0 | 0 |
| cliproxy | 8 | 0 | 0 |
| cpa-common | 2 | 0 | 0 |
| cpa-home | 2 | 0 | 0 |

- **Value and Mapping**: `Value` (Null, Bool, Number, String, Sequence, Mapping,
  Tagged) with `as_bool`, `as_str`, `as_i64`, `as_u64`, `as_sequence(_mut)`,
  `as_mapping(_mut)`, `Value::from`; a `Mapping` with Value keys in source order
  (`Mapping::new`, `is_empty`, insertion order kept by the comment-preserving
  rewrites in `config/document.rs`); `value::TaggedValue` and `Tag::new`, which
  cpa-core uses for its timestamp tag `!cpa-timestamp`.
- **Text to Value**: `from_str::<Value>` (most uses), `from_slice` (cpa-exec
  antigravity_version), `Deserializer::from_str` (one multi-document read).
- **Typed decoding**: mostly `from_value` from an already-corrected `Value`
  (`config.rs` builds `Config` with `from_value(null_scalars_to_zero(..))`,
  `registry/dynamic.rs`, `cpa-server/capabilities.rs`); `from_str::<T>` only in a few
  places and tests (`ThinkingSupport`).
- **Value to text**: `to_string` (six sites) and cpa-core's `yaml_string`, which
  post-processes it and whose output `config/document.rs` writes into rewritten
  config files.
- **`Error`**: only displayed, never matched on.
- **Events** (`config/binary.rs`, `events_with`): stream, document-start (its span, for
  the `%00` check), scalar (value bytes, anchor name, resolved tag, plain or not,
  start and end byte index), alias (anchor name), sequence and mapping start (anchor,
  tag) and end. They drive `!!binary` decoding, local-tag reads, the refusals of
  repeated anchors and tagged keys, and the list of plain scalars `parse_yaml`
  rewrites.
- **yaml-edit 0.3.2** (comment-preserving rewrites, and `lex` to find TAG and DIRECTIVE
  tokens) depends only on rowan and base64. It has its own parser, does not use
  libyaml, and stays.

## Candidates

| | serde_yaml_ng 0.10 (today) | serde-saphyr 2.1.0 + granit-parser 2.1.0 | noyalib 0.0.58 | saphyr 0.3.3 (+ saphyr-parser) |
|---|---|---|---|---|
| unsafe | unsafe-libyaml (C translated to Rust) | `forbid(unsafe_code)` in both | `forbid(unsafe_code)` | no `unsafe` (no forbid attribute) |
| latest release | 2024-05-26 | 2026-11-03 / 2026-11-02 | 2026-10-15 | 2026-10-27 |
| last commit | 2025-09 | 2026-11-02 / 2026-11-02 | 2026-10-15 | 2026-10-27 |
| stars, downloads | 21.7M downloads | 242 stars, 15.6M / 12 stars, 12.4M | 11 stars, 1.1M | 339 stars, 2.8M |
| MSRV | | 1.89 / 1.81 | 1.89 | 1.85 |
| normal deps (transitive) | 16 incl. unsafe-libyaml | 18 / 2 | 13 | 18 |
| min. from_str + to_string binary | 395 KB | 1 405 KB | 669 KB | (no serde) |
| ordered Value / Mapping | yes, Value keys | none (cpa-core owns one) | yes, but `Mapping` has String keys only (`MappingAny` separate) | `Yaml` with LinkedHashMap, Value keys, no serde |
| serde from_str / from_value | yes / yes | yes / no Value | yes / yes | no |
| to_string | single quotes | double quotes, quotes `y`/`n` | double quotes, indented sequences | `YamlEmitter` |
| events with tags | via unsafe-libyaml | yes: resolved tags, styles, anchor names, byte spans, comments, `%YAML` version | no public event API | yes, but char indices and anchor ids only |
| merge keys, `!!binary` | no, no | yes, yes | yes, no | no, no |
| duplicate keys | error | error (policy option) | accepted | error |

The binary sizes are release builds of a `main` that parses its argument into
`serde_json::Value` and prints it back, measured on Windows x64 with each library
alone. Dependency counts are `cargo tree -e normal` with default features.

## Corpus differences from yaml.v3

`crates/cpa-core/tests/fixtures/yaml_v3/` holds 119 edge cases (YAML 1.1 and 1.2
booleans, octal, hex, binary and underscored numbers, `.inf` and `.nan`, null forms,
timestamps, anchors and aliases, merge keys, standard, custom, non-specific and
verbatim tags, `%TAG` and `%YAML` directives, duplicate keys, multi-document streams,
flow and block styles, block scalars with chomping, escapes, comments, BOM, tabs,
CR and CRLF, NEL, deep nesting, complex keys and typed-field cases). The recording
also replays every `yaml` input of cpa-core's other `*_go.json` fixtures (199) and
Go's `config.example.yaml`.

Each cell says how the parser's untyped result compares with `yaml.Unmarshal` into
`interface{}`: `ok` (same value, or both fail), `value` (both parse, values differ),
`Go err` (Go fails, the parser does not) or `Rust err`. Rows where every column is
`ok` are left out. `serde_yaml_ng` and `parse_yaml` come from the committed test
(`parse_yaml` is cpa-core's Go-corrected path with `expand_merges`); the other
columns come from the throwaway harness, through a serde `deserialize_any` visitor
(saphyr through its own `Yaml`). `serde-saphyr+opts` is serde-saphyr with
`strict_booleans`, `legacy_octal_numbers`, `NonFiniteFloatPolicy::PassThrough` and
`budget.max_depth = 10_000`.

| case | serde_yaml_ng | parse_yaml | serde-saphyr | serde-saphyr+opts | noyalib | saphyr |
|---|---|---|---|---|---|---|
| anchors_aliases | ok | ok | value | ok | ok | ok |
| bom | Rust err | ok | ok | ok | ok | ok |
| bom_doc_start | Rust err | ok | ok | ok | ok | ok |
| bool_case_forms | ok | ok | value | ok | ok | ok |
| bool_yaml11_words | ok | ok | value | ok | ok | ok |
| config_like | value | ok | value | ok | value | value |
| deep_block_150 | Rust err | Rust err | Rust err | ok | Rust err | ok |
| deep_flow_200 | Rust err | Rust err | Rust err | ok | Rust err | ok |
| deep_flow_64 | ok | ok | Rust err | ok | ok | ok |
| dup_keys | ok | ok | ok | ok | Go err | ok |
| dup_keys_flow | ok | ok | ok | ok | Go err | ok |
| dup_keys_int_forms | Rust err | Rust err | Rust err | Rust err | value | Rust err |
| dup_keys_nested | ok | ok | ok | ok | Go err | ok |
| dup_keys_null_spellings | Rust err | Rust err | value | value | value | Rust err |
| dup_keys_quoted_vs_plain | ok | ok | ok | ok | Go err | ok |
| float_forms | value | ok | value | value | value | value |
| float_specials | ok | ok | Rust err | ok | ok | ok |
| indentation_odd | ok | ok | value | ok | ok | ok |
| int_forms | value | ok | value | value | value | value |
| int_ranges | Rust err | ok | ok | ok | value | value |
| key_complex | Go err | Go err | Go err | Go err | Go err | Go err |
| key_mapping | Go err | Go err | Go err | Go err | Go err | Go err |
| merge_after_key | value | ok | value | ok | ok | value |
| merge_inline | value | ok | ok | ok | ok | value |
| merge_list | value | ok | value | ok | ok | value |
| merge_nested_chain | value | ok | value | ok | ok | value |
| merge_quoted_key | ok | value | ok | ok | ok | ok |
| merge_scalar_error | Go err | ok | ok | ok | ok | Go err |
| merge_single | value | ok | value | ok | ok | value |
| multi_doc | Rust err | Rust err | Rust err | Rust err | Rust err | ok |
| multi_doc_empty_first | Rust err | Rust err | Rust err | Rust err | Rust err | ok |
| multi_doc_markers | Rust err | Rust err | Rust err | Rust err | Rust err | ok |
| nel_in_plain | ok | ok | Go err | Go err | Go err | Go err |
| quoting_slash_escape | Go err | Go err | Go err | Go err | Go err | Go err |
| tab_after_seq | ok | ok | Go err | Go err | Go err | Go err |
| tabs_in_value | ok | ok | ok | ok | ok | Rust err |
| tag_binary | value | value | ok | ok | value | value |
| tag_binary_bad | Go err | Go err | ok | ok | Go err | Go err |
| tag_bool_quoted | ok | ok | ok | ok | ok | value |
| tag_bool_yes | ok | ok | Go err | ok | ok | Go err |
| tag_custom_mapping | value | value | ok | ok | value | value |
| tag_custom_scalar | value | value | ok | ok | value | value |
| tag_directive | ok | ok | ok | ok | value | value |
| tag_int_bad | ok | ok | ok | ok | ok | Go err |
| tag_int_quoted | ok | ok | ok | ok | ok | value |
| tag_nonspecific | value | value | value | value | value | value |
| tag_null | Rust err | Rust err | ok | ok | ok | value |
| tag_omap | ok | ok | ok | ok | value | ok |
| tag_set | ok | ok | ok | ok | value | ok |
| tag_verbatim | ok | ok | ok | ok | ok | value |
| timestamp_tagged | value | value | Rust err | Rust err | value | value |
| timestamps | value | ok | value | value | value | value |
| typed_bool_on | ok | ok | value | ok | ok | ok |
| typed_bool_y | ok | ok | value | ok | ok | ok |
| typed_bool_yes | ok | ok | value | ok | ok | ok |
| typed_float_inf | ok | ok | Rust err | ok | ok | ok |
| typed_int_octal11 | value | ok | value | ok | value | value |
| typed_int_overflow | Rust err | ok | ok | ok | ok | ok |
| typed_int_under | value | ok | ok | ok | value | value |
| typed_string_timestamp | value | ok | value | value | value | value |
| yaml_directive_11 | value | ok | value | ok | value | value |
| **corpus differences (of 119)** | **35** | **18** | **36** | **16** | **34** | **36** |
| **fixture differences (of 199)** | **105** | **95** | **30** | **25** | **100** | **107** |

### Typed decoding

The same inputs decoded into a struct with the field kinds of Go's config (string,
int, bool, float64, []string, map[string]string, nested struct). Only cases with a
difference are listed.

| case | serde_yaml_ng | serde-saphyr | serde-saphyr+opts | noyalib |
|---|---|---|---|---|
| bom | Rust err | ok | ok | ok |
| bom_doc_start | Rust err | ok | ok | ok |
| config_like | Rust err | ok | Rust err | Rust err |
| deep_block_150 | ok | Rust err | ok | Rust err |
| deep_flow_200 | ok | Rust err | ok | Rust err |
| deep_flow_64 | ok | Rust err | ok | ok |
| dup_keys | Go err | ok | ok | Go err |
| dup_keys_flow | Go err | ok | ok | Go err |
| dup_keys_int_forms | ok | Rust err | Rust err | ok |
| dup_keys_nested | ok | Rust err | Rust err | ok |
| dup_keys_null_spellings | ok | Rust err | Rust err | ok |
| dup_keys_quoted_vs_plain | Go err | ok | ok | Go err |
| empty_document | ok | Rust err | Rust err | ok |
| float_specials | ok | Rust err | ok | ok |
| key_complex | ok | ok | ok | Go err |
| key_mapping | ok | ok | ok | Go err |
| merge_scalar_error | ok | Rust err | Rust err | ok |
| multi_doc | Rust err | Rust err | Rust err | Rust err |
| multi_doc_empty_first | Rust err | Rust err | Rust err | Rust err |
| multi_doc_markers | Rust err | Rust err | Rust err | Rust err |
| nel_in_plain | ok | Go err | Go err | Go err |
| null_key | ok | Rust err | Rust err | ok |
| null_root | Rust err | Rust err | Rust err | ok |
| only_comment | ok | Rust err | Rust err | ok |
| only_doc_start | ok | Rust err | Rust err | ok |
| quoting_slash_escape | Go err | Go err | Go err | Go err |
| tag_binary_bad | ok | Rust err | Rust err | ok |
| tag_bool_yes | ok | ok | Rust err | ok |
| tag_int_bad | ok | Rust err | Rust err | ok |
| timestamp_tagged | ok | Rust err | Rust err | ok |
| typed_bool_on | Rust err | ok | Rust err | Rust err |
| typed_bool_quoted | ok | Go err | Go err | ok |
| typed_bool_y | Rust err | ok | Rust err | Rust err |
| typed_bool_yes | Rust err | ok | Rust err | Rust err |
| typed_int_float | Rust err | Rust err | Rust err | ok |
| typed_int_null | Rust err | Rust err | Rust err | Rust err |
| typed_int_octal11 | Rust err | Rust err | ok | value |
| typed_int_quoted | ok | Go err | Go err | ok |
| typed_int_under | Rust err | ok | ok | Rust err |
| typed_map_null_value | value | Rust err | Rust err | Rust err |
| typed_seq_mixed | value | Rust err | Rust err | Rust err |
| typed_string_bool | ok | ok | ok | Rust err |
| typed_string_float | ok | ok | ok | Rust err |
| typed_string_hex | ok | ok | ok | Rust err |
| typed_string_int | ok | ok | ok | Rust err |
| typed_string_null | value | Rust err | Rust err | Rust err |
| **total** | **21** | **29** | **29** | **26** |

serde-saphyr's typed path is stricter than Go's: it refuses a null into a `String` or
`i64`, an empty document into a struct, and, with `strict_booleans`, `yes`, `on` and
`y` into a `bool`, all of which Go reads as zero values or `true`. This does not
decide the choice, because cpa-core builds `Config` with `from_value` from its own
corrected `Value`, whose `Deserializer` the wrapper owns.

## Known differences of the chosen stack

From yaml.v3, with serde-saphyr+opts, before the wrapper's own resolution:

- `y`, `Y`, `n`, `N`, `yes`, `on` and the other YAML 1.1 words are strings untyped (as
  in Go), but `strict_booleans` also refuses them for `bool` fields, which Go accepts.
  The wrapper's `Value` deserializer applies `go_bool` as today.
- `017` reads as 15 and `08` as 8 (Go reads `08` as the float 8), `1__0` and `0x_1F`
  stay strings where Go reads 10 and 31, and `685.230_15e+03` and `1_000.5` stay
  strings where Go reads floats.
- Timestamps (`2001-12-14`, `2001-12-14 21:59:43.10 -5`) are strings, where Go reads a
  `time.Time` into `interface{}`; `!!timestamp` is refused. cpa-core's
  `!cpa-timestamp` tagging must move into the wrapper.
- A non-specific tag (`! 12`) reads as the string `12`; Go resolves it as untagged
  (int 12).
- A custom local tag on a number (`!keep 0x1F`) reads as 31; Go reads the text `0x1F`.
- `!!binary` with non-canonical padding (`YR==`) or non-UTF-8 content is refused; Go
  decodes it. A space inside the base64 (`'c2Vj cmV0'`) is accepted; Go refuses it.
- A merge whose operand is an aliased sequence, and collection keys (`? [a, b]`), are
  accepted; Go fails. NEL (U+0085) line breaks are read differently: `nel_in_plain`
  and two NEL-after-comment fixtures fail in Go and parse here, two others parse in
  Go and fail here. A tab after `-` and the escape `\/` are accepted; Go refuses them.
- `1: a` and `0x1: b` in one mapping are a duplicate-key error (Go compares key text);
  `~: 1` and `null: 2` are two keys where Go keeps the second.
- `from_str` refuses a stream with more than one document (as serde_yaml_ng does); Go
  reads the first. `from_multiple` reads all.

Building `Value` from granit-parser events with a port of yaml.v3 `resolve()`
removes the first six groups by construction; the rest are parser-level and must be
listed in `docs/DIFFERENCES-FROM-GO.md` or refused, as binary.rs refuses today.

From today's serde_yaml_ng plus cpa-core corrections (`parse_yaml`): the corpus cases
that serde_yaml_ng gets wrong and cpa-core corrects (BOM, numbers, merges,
timestamps, `%YAML 1.1` octal) come out right from serde-saphyr already, as do
`!!binary`, custom-tag text and `!!null ""`. The deep-nesting
cases that serde_yaml_ng refuses (recursion limit 128) parse with the raised budget;
the wrapper should keep a depth limit (Go's is 10 000 flow levels).

## Migration plan

1. **cpa-core: `cpa_core::yaml`** (bead .44.2, the bulk of the work, about 1 to 1.5k
   lines changed). Expose:
   - `Value`, `Mapping` (an `indexmap::IndexMap<Value, Value>`), `Number`,
     `TaggedValue`/`Tag`, with the accessor names other crates use today
     (`as_bool`, `as_str`, `as_i64`, `as_u64`, `as_sequence(_mut)`,
     `as_mapping(_mut)`, `From` impls, `Index` by `&str`), plus `Serialize`,
     `Deserialize` and an `IntoDeserializer` so `from_value` keeps working;
   - `from_str::<Value>` built from granit-parser events with a port of yaml.v3
     `resolve.go`, merges and `!!binary`, replacing `parse_yaml`'s rewrite pass,
     `expand_merges` and most of binary.rs; `from_str_multi`;
   - `from_value<T>`, `to_value<T>`;
   - `to_string(&Value)` that reproduces today's `yaml_string` text (pin it with the
     existing `yaml_string` tests; either configure serde-saphyr's
     `SerializerOptions` or emit scalars in the wrapper);
   - `Error` (Display only);
   - the event iterator binary.rs uses (`events_with`, `plain_scalars`) re-implemented
     on granit-parser, with anchor-name events on and byte offsets as they are;
   - then drop `unsafe-libyaml` from cpa-core's Cargo.toml.
   Done when `yaml_v3_go` passes with a re-blessed, shorter known-differences list,
   and the existing goldens (`binary_strings_go`, `prepare_upload_go`,
   `scalar_settings`, `document.rs` round trips) pass unchanged.
2. **Other crates** (bead .44.3, mechanical): replace `serde_yaml_ng::` with
   `cpa_core::yaml::` (imports such as `use serde_yaml_ng::Value as Yaml` become
   `use cpa_core::yaml::Value as Yaml`): cpa-server (127 lines: management views,
   plugins, plugin store, realtime relay, tests), cpa-exec (25: Claude settings,
   antigravity version), cpa-plugin (10), cliproxy (8), cpa-common (2), cpa-home (2).
   cpa-store and cpa-translate do not use YAML. Then remove `serde_yaml_ng` from the
   workspace, and check `cargo audit` no longer lists unsafe-libyaml.
3. Nothing outside cpa-core depends on serde-saphyr or granit-parser directly, so a
   later parser change touches one module.

## Reproducing

The corpus, recorder and regeneration command are in
`crates/cpa-core/tests/reference/yaml_v3/README.md`. The test is
`cargo test -p cpa-core --test yaml_v3_go`. The candidate harness was throwaway code
outside the repository; its tables above were produced from the same recording.
