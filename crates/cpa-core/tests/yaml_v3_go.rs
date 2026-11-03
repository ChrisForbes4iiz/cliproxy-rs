//! The YAML parser against gopkg.in/yaml.v3 v3.0.1, the version CLIProxyAPI v8.0.35
//! pins, on the edge-case corpus in `fixtures/yaml_v3/` plus every config.yaml-shaped
//! input of the other `fixtures/*_go.json` files and Go's config.example.yaml, as
//! recorded by `tests/reference/yaml_v3/main.go` into `fixtures/yaml_v3_go.json`.
//!
//! It does not assert parity: [`cpa_core::yaml`] differs from yaml.v3 in known ways
//! (some of which the config loader corrects or refuses). It asserts that the
//! differences are exactly the ones listed in `fixtures/yaml_v3_known_differences.json`,
//! so a parser change (cliproxy-rs-7ea.44) shows every case it fixes or breaks. After a
//! deliberate change, regenerate the list with `YAML_V3_BLESS=1` and review its diff.
//!
//! Modes, each compared with Go's recording of the same case:
//! - `yaml`: `cpa_core::yaml::from_str::<Value>`, the raw parser;
//! - `parse_yaml`: [`cpa_core::config::parse_yaml`] then [`cpa_core::config::expand_merges`],
//!   the untyped path the config loader builds on;
//! - `stream`: every document through `cpa_core::yaml::from_str_multi`, against a
//!   yaml.v3 `Decoder`;
//! - `typed`: `cpa_core::yaml::from_str::<Shape>`, a struct with the field kinds of Go's
//!   config, against yaml.v3 decoding the same struct.
//!
//! A difference is `value` (both parse, the values differ), `go-error` (Go fails, Rust
//! parses) or `rust-error` (Rust fails, Go parses). Two errors count as agreement; error
//! texts are not compared.
use std::collections::BTreeMap;
use std::path::PathBuf;

use cpa_core::yaml::Value;
use serde::Deserialize;
use serde_json::{Value as Json, json};

/// A value as flat pre-order `[type, text]` tokens, as the Go recorder writes them.
type Tokens = Vec<[String; 2]>;

fn tok(kind: &str, text: impl Into<String>) -> [String; 2] {
    [kind.to_owned(), text.into()]
}

/// Go's `strconv.FormatFloat(f, 'g', -1, 64)` spellings for the non-finite values;
/// finite ones are compared as numbers.
fn float_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "+Inf" } else { "-Inf" }.into()
    } else {
        f.to_string()
    }
}

/// The recorder's map entry order: key tokens joined with \x01 and \x02, bytewise.
fn sort_key(tokens: &[[String; 2]]) -> String {
    tokens.iter().map(|[k, t]| format!("{k}\x01{t}\x02")).collect()
}

fn encode(value: &Value, out: &mut Tokens) {
    match value {
        Value::Null => out.push(tok("null", "")),
        Value::Bool(b) => out.push(tok("bool", b.to_string())),
        Value::Number(n) => out.push(match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => tok("int", i.to_string()),
            (None, Some(u), _) => tok("int", u.to_string()),
            (None, None, f) => tok("float", float_text(f.unwrap_or(f64::NAN))),
        }),
        Value::String(s) => out.push(tok("str", s.as_str())),
        Value::Sequence(items) => {
            out.push(tok("seq", items.len().to_string()));
            items.iter().for_each(|item| encode(item, out));
        }
        Value::Mapping(map) => {
            let mut entries: Vec<(String, Tokens)> = map
                .iter()
                .map(|(k, v)| {
                    let mut key = Vec::new();
                    encode(k, &mut key);
                    let order = sort_key(&key);
                    encode(v, &mut key);
                    (order, key)
                })
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            out.push(tok("map", entries.len().to_string()));
            entries.into_iter().for_each(|(_, t)| out.extend(t));
        }
        Value::Tagged(tagged) => match &tagged.value {
            Value::String(s) if tagged.tag == cpa_core::config::TIMESTAMP_TAG => out.push(tok("timestamp", s.as_str())),
            inner => {
                out.push(tok("tagged", tagged.tag.to_string()));
                encode(inner, out);
            }
        },
    }
}

fn tokens(value: &Value) -> Tokens {
    let mut out = Vec::new();
    encode(value, &mut out);
    out
}

fn same_token(go: &[String; 2], rust: &[String; 2]) -> bool {
    if go[0] != rust[0] {
        return false;
    }
    if go[0] == "float" {
        let num = |s: &str| s.parse::<f64>().ok();
        return match (num(&go[1]), num(&rust[1])) {
            (Some(a), Some(b)) => a == b || (a.is_nan() && b.is_nan()),
            _ => go[1] == rust[1],
        };
    }
    go[1] == rust[1]
}

fn same(go: &Json, rust: &Tokens) -> bool {
    let go: Tokens = serde_json::from_value(go.clone()).expect("recorded tokens");
    go.len() == rust.len() && go.iter().zip(rust).all(|(g, r)| same_token(g, r))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Nested {
    name: String,
    enabled: bool,
}

/// The field kinds of Go's `internal/config.Config`, as the recorder's `shape`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Shape {
    host: String,
    port: i64,
    debug: bool,
    #[serde(rename = "api-keys")]
    api_keys: Option<Vec<String>>,
    #[serde(rename = "proxy-url")]
    proxy_url: String,
    #[serde(rename = "request-retry")]
    request_retry: i64,
    ratio: f64,
    headers: Option<BTreeMap<String, String>>,
    nested: Nested,
}

fn shape_json(s: Shape) -> Json {
    json!({
        "host": s.host, "port": s.port.to_string(), "debug": s.debug, "api-keys": s.api_keys,
        "proxy-url": s.proxy_url, "request-retry": s.request_retry.to_string(),
        "ratio": float_text(s.ratio), "headers": s.headers,
        "nested": {"name": s.nested.name, "enabled": s.nested.enabled},
    })
}

fn same_shape(go: &Json, rust: &Json) -> bool {
    let (Some(go), Some(rust)) = (go.as_object(), rust.as_object()) else {
        return false;
    };
    go.iter().all(|(k, v)| {
        let r = rust.get(k).unwrap_or(&Json::Null);
        if k == "ratio" {
            same_token(
                &tok("float", v.as_str().unwrap_or("")),
                &tok("float", r.as_str().unwrap_or("")),
            )
        } else {
            v == r
        }
    })
}

fn kind<T>(go_error: bool, rust: &Result<T, String>, equal: impl FnOnce(&T) -> bool) -> Option<&'static str> {
    match (go_error, rust) {
        (true, Err(_)) => None,
        (true, Ok(_)) => Some("go-error"),
        (false, Err(_)) => Some("rust-error"),
        (false, Ok(v)) => (!equal(v)).then_some("value"),
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

type Differences = BTreeMap<String, BTreeMap<String, String>>;

fn differences() -> Differences {
    let cases: Vec<Json> = serde_json::from_str(include_str!("fixtures/yaml_v3_go.json")).expect("recording");
    let mut out = Differences::new();
    let mut note = |mode: &str, name: &str, kind: Option<&str>| {
        if let Some(kind) = kind {
            out.entry(mode.to_owned())
                .or_default()
                .insert(name.to_owned(), kind.to_owned());
        }
    };
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let yaml = case["yaml"].as_str().unwrap();
        let go_error = case.get("error").is_some();
        let go_value = &case["value"];

        let raw = cpa_core::yaml::from_str::<Value>(yaml).map_err(|e| e.to_string());
        note("yaml", name, kind(go_error, &raw, |v| same(go_value, &tokens(v))));

        let cpa = cpa_core::config::parse_yaml(yaml)
            .map_err(|e| e.to_string())
            .and_then(|mut v| {
                cpa_core::config::expand_merges(&mut v).map_err(|e| e.to_string())?;
                Ok(v)
            });
        note("parse_yaml", name, kind(go_error, &cpa, |v| same(go_value, &tokens(v))));

        let stream: Result<Vec<Value>, String> = cpa_core::yaml::from_str_multi(yaml).map_err(|e| e.to_string());
        let go_stream = case["stream"].as_array().unwrap();
        note(
            "stream",
            name,
            kind(case.get("stream_error").is_some(), &stream, |docs| {
                docs.len() == go_stream.len() && go_stream.iter().zip(docs).all(|(g, d)| same(g, &tokens(d)))
            }),
        );

        let typed = cpa_core::yaml::from_str::<Shape>(yaml)
            .map(shape_json)
            .map_err(|e| e.to_string());
        note(
            "typed",
            name,
            kind(case.get("typed_error").is_some(), &typed, |v| {
                same_shape(&case["typed"], v)
            }),
        );
    }
    out
}

#[test]
fn yaml_parser_differences_from_go_yaml_v3_are_the_known_ones() {
    let actual = differences();
    let path = fixtures().join("yaml_v3_known_differences.json");
    if std::env::var_os("YAML_V3_BLESS").is_some() {
        let text = serde_json::to_string_pretty(&actual).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        return;
    }
    let known: Differences =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("known differences")).expect("known differences");
    let mut report = Vec::new();
    for mode in known
        .keys()
        .chain(actual.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let (k, a) = (
            known.get(mode).cloned().unwrap_or_default(),
            actual.get(mode).cloned().unwrap_or_default(),
        );
        for (name, kind) in &a {
            if k.get(name) != Some(kind) {
                report.push(format!("{mode}: {name}: now {kind}, listed {:?}", k.get(name)));
            }
        }
        for name in k.keys().filter(|n| !a.contains_key(*n)) {
            report.push(format!("{mode}: {name}: now matches Go, listed {:?}", k[name]));
        }
    }
    assert!(
        report.is_empty(),
        "differences from yaml.v3 changed (YAML_V3_BLESS=1 rewrites the list):\n{}",
        report.join("\n")
    );
}

/// The corpus files and the recording agree, so a corpus edit cannot go unrecorded.
#[test]
fn yaml_v3_recording_covers_the_corpus() {
    let cases: Vec<Json> = serde_json::from_str(include_str!("fixtures/yaml_v3_go.json")).unwrap();
    let recorded: BTreeMap<&str, &str> = cases
        .iter()
        .filter_map(|c| {
            let name = c["name"].as_str()?.strip_prefix("corpus/")?;
            Some((name, c["yaml"].as_str()?))
        })
        .collect();
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(fixtures().join("yaml_v3")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "yaml") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            files.insert(name, std::fs::read(&path).unwrap());
        }
    }
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        recorded.keys().copied().collect::<Vec<_>>()
    );
    for (name, bytes) in &files {
        assert_eq!(
            recorded[name.as_str()].as_bytes(),
            bytes.as_slice(),
            "{name}: re-run the recorder"
        );
    }
}
