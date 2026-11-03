//! YAML for every cliproxy-rs crate, in safe Rust (docs/YAML-PARSER.md).
//!
//! Text is parsed by granit-parser and composed here, with scalars resolved as
//! gopkg.in/yaml.v3 v3.0.1 resolves them (see [`load`] for the rules), into an ordered
//! [`Value`] whose API is serde_yaml_ng 0.10's (the `value`, `mapping` and `number`
//! modules are adapted from serde_yaml_ng, MIT licensed). [`to_string`] writes what
//! serde_yaml_ng wrote. Other crates use this module and never the parser itself, so
//! a later parser change touches cpa-core only.

mod emit;
mod error;
pub(crate) mod load;
pub mod mapping;
mod ng;
mod number;
pub mod value;

pub use error::Error;
pub use load::from_str_multi;
pub(crate) use load::go_base64;
pub use mapping::Mapping;
pub use number::Number;
pub use value::{Index, Sequence, Tag, TaggedValue, Value, from_value, to_value};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Prevents other crates from implementing [`Index`].
mod private {
    pub trait Sealed {}
    impl Sealed for usize {}
    impl Sealed for str {}
    impl Sealed for String {}
    impl Sealed for super::Value {}
    impl<T> Sealed for &T where T: ?Sized + Sealed {}
}

/// The first document of `text` as `T`. A [`Value`] resolves its scalars as yaml.v3
/// does into `interface{}`; any other `T` decodes as yaml.v3 decodes into a typed Go
/// value (a string field takes a scalar's text, null is a zero value).
pub fn from_str<T: DeserializeOwned + 'static>(text: &str) -> Result<T, Error> {
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<Value>() {
        let value: Box<dyn std::any::Any> = Box::new(load::from_str(text)?);
        return Ok(*value.downcast::<T>().expect("T is Value"));
    }
    load::from_str_typed(text)
}

/// [`from_str`] of UTF-8 `bytes`.
pub fn from_slice<T: DeserializeOwned + 'static>(bytes: &[u8]) -> Result<T, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::msg(format!("invalid UTF-8: {e}")))?;
    from_str(text)
}

/// `value` as one YAML document, written as serde_yaml_ng 0.10 wrote it.
pub fn to_string<T: ?Sized + Serialize>(value: &T) -> Result<String, Error> {
    emit::to_string(&to_value(value)?)
}
