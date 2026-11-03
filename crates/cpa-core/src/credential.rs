//! Runtime credentials.
//!
//! A credential keeps its complete JSON metadata so fields this crate does not know
//! survive a write-back. Providers read it through their own validated views; this type
//! carries only what routing and persistence need (sdk/cliproxy/auth/types.go).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A JSON file in `auth-dir`.
    File(PathBuf),
    /// An entry synthesized from `config.yaml` (for example `claude-api-key[i]`).
    Config { section: String, index: usize },
    /// A runtime-only credential with no file or config entry (Go `runtime_only`): an
    /// AI Studio browser connected to the `/v1/ws` relay, for as long as it stays
    /// connected.
    Runtime,
}

/// `Debug` is written by hand: metadata and attributes hold tokens and API keys, so
/// only their keys are shown.
#[derive(Clone)]
pub struct Credential {
    /// Path relative to `auth-dir` for files, as CLIProxyAPI uses.
    pub id: String,
    /// The `type` field: `claude`, `codex`, `antigravity`, ...
    pub provider: String,
    pub source: Source,
    pub disabled: bool,
    /// Email when present, otherwise the provider.
    pub label: String,
    /// Config-derived attributes, never persisted to the credential file.
    pub attributes: BTreeMap<String, String>,
    /// The full credential JSON. Source of truth for provider fields.
    pub metadata: Map<String, Value>,
    /// Store-wide monotonic stamp set on every accepted change, never reused even after
    /// a delete and re-create. Patches against an old revision are rejected.
    pub revision: u64,
    /// Go `NextRefreshAfter` as the plugin auth parser returned it for this credential.
    /// Registering the credential takes it per Go's registration rule; never persisted.
    pub next_refresh_after: Option<std::time::SystemTime>,
}

/// Map keys without their values, for `Debug`.
struct KeysOnly<'a>(Vec<&'a str>);

impl<'a> KeysOnly<'a> {
    fn of(keys: impl Iterator<Item = &'a String>) -> Self {
        Self(keys.map(String::as_str).collect())
    }
}

impl std::fmt::Debug for KeysOnly<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(&self.0).finish()
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("source", &self.source)
            .field("disabled", &self.disabled)
            .field("label", &self.label)
            .field("attributes", &KeysOnly::of(self.attributes.keys()))
            .field("metadata", &KeysOnly::of(self.metadata.keys()))
            .field("revision", &self.revision)
            .field("next_refresh_after", &self.next_refresh_after)
            .finish()
    }
}

impl Credential {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).and_then(Value::as_str)
    }

    /// The runtime-only `aistudio` credential of a `/v1/ws` relay session (Go
    /// `wsOnConnected`): the channel ID is its ID, label and metadata email.
    pub fn relay_session(channel: &str) -> Self {
        let mut metadata = Map::new();
        metadata.insert("email".into(), Value::String(channel.to_owned()));
        Self {
            id: channel.to_owned(),
            provider: "aistudio".into(),
            source: Source::Runtime,
            disabled: false,
            label: channel.to_owned(),
            attributes: BTreeMap::from([("runtime_only".to_owned(), "true".to_owned())]),
            metadata,
            revision: 0,
            next_refresh_after: None,
        }
    }

    /// Builds a credential from one auth file. Returns `None` for files CLIProxyAPI
    /// ignores here: no `type`, or `gemini-cli` (internal/watcher/synthesizer/file.go).
    pub fn from_file(auth_dir: &Path, path: &Path, metadata: Map<String, Value>) -> Option<Self> {
        let provider = metadata
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if provider.is_empty() || provider == "gemini-cli" {
            return None;
        }
        let id = path
            .strip_prefix(auth_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let mut cred = Self {
            id,
            disabled: false,
            label: String::new(),
            provider,
            source: Source::File(path.to_owned()),
            attributes: BTreeMap::new(),
            metadata,
            revision: 0,
            next_refresh_after: None,
        };
        cred.refresh_derived();
        Some(cred)
    }

    /// Recomputes the fields derived from `metadata`. Call after every metadata change.
    pub fn refresh_derived(&mut self) {
        self.disabled = self.metadata.get("disabled").and_then(Value::as_bool).unwrap_or(false);
        self.label = match self.str("email") {
            Some(email) if !email.is_empty() => email.to_owned(),
            _ => self.provider.clone(),
        };
    }
}

/// A change to credential metadata, produced by token refresh or the management API.
/// `Debug` shows the keys it sets, not their values (refreshed tokens).
#[derive(Clone, Default)]
pub struct MetadataPatch {
    pub set: Map<String, Value>,
    pub remove: Vec<String>,
}

impl std::fmt::Debug for MetadataPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataPatch")
            .field("set", &KeysOnly::of(self.set.keys()))
            .field("remove", &self.remove)
            .finish()
    }
}

impl MetadataPatch {
    pub fn apply(&self, metadata: &mut Map<String, Value>) {
        for key in &self.remove {
            metadata.remove(key);
        }
        for (key, value) in &self.set {
            metadata.insert(key.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;

    /// cliproxy-rs-7ea.35: `{:?}` of a credential or a patch never shows a token.
    #[test]
    fn debug_output_redacts_secret_values() {
        const SECRET: &str = "sk-fake-debug-secret-0123";
        let mut metadata = Map::new();
        metadata.insert("type".into(), Value::from("claude"));
        metadata.insert("access_token".into(), Value::from(SECRET));
        metadata.insert("refresh_token".into(), Value::from(SECRET));
        let mut cred = Credential::from_file(Path::new("/auth"), Path::new("/auth/a.json"), metadata.clone()).unwrap();
        cred.attributes.insert("api_key".into(), SECRET.into());
        let shown = format!("{cred:?} {cred:#?}");
        assert!(!shown.contains(SECRET), "{shown}");
        for visible in ["a.json", "claude", "access_token", "api_key"] {
            assert!(shown.contains(visible), "{visible}: {shown}");
        }
        let patch = MetadataPatch {
            set: metadata,
            remove: vec!["expired".into()],
        };
        let shown = format!("{patch:?}");
        assert!(!shown.contains(SECRET), "{shown}");
        assert!(shown.contains("refresh_token") && shown.contains("expired"), "{shown}");
    }
}
