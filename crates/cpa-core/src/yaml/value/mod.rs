//! The Value enum, a loosely typed way of representing any valid YAML value.

mod de;
mod debug;
mod from;
mod index;
mod partial_eq;
mod ser;
pub(crate) mod tagged;

use crate::yaml::error::{self, Error, ErrorImpl};
use serde::Serialize;
use serde::de::{Deserialize, DeserializeOwned, IntoDeserializer};
use std::hash::{Hash, Hasher};
use std::mem;

pub use self::index::Index;
pub use self::ser::Serializer;
pub use self::tagged::{Tag, TaggedValue};
#[doc(inline)]
pub use crate::yaml::mapping::Mapping;
pub use crate::yaml::number::Number;

/// Represents any valid YAML value.
#[derive(Clone, PartialEq, PartialOrd)]
pub enum Value {
    /// Represents a YAML null value.
    Null,
    /// Represents a YAML boolean.
    Bool(bool),
    /// Represents a YAML numerical value, whether integer or floating point.
    Number(Number),
    /// Represents a YAML string.
    String(String),
    /// Represents a YAML sequence in which the elements are
    /// `cpa_core::yaml::Value`.
    Sequence(Sequence),
    /// Represents a YAML mapping in which the keys and values are both
    /// `cpa_core::yaml::Value`.
    Mapping(Mapping),
    /// A representation of YAML's `!Tag` syntax, used for enums.
    Tagged(Box<TaggedValue>),
}

/// The default value is `Value::Null`.
///
/// This is useful for handling omitted `Value` fields when deserializing.
impl Default for Value {
    fn default() -> Value {
        Value::Null
    }
}

/// A YAML sequence in which the elements are `cpa_core::yaml::Value`.
pub type Sequence = Vec<Value>;

/// Convert a `T` into `cpa_core::yaml::Value` which is an enum that can represent
/// any valid YAML data.
///
/// This conversion can fail if `T`'s implementation of `Serialize` decides to
/// return an error.
pub fn to_value<T>(value: T) -> Result<Value, Error>
where
    T: Serialize,
{
    value.serialize(Serializer)
}

/// Interpret a `cpa_core::yaml::Value` as an instance of type `T`.
///
/// This conversion can fail if the structure of the Value does not match the
/// structure expected by `T`, for example if `T` is a struct type but the Value
/// contains something other than a YAML map. It can also fail if the structure
/// is correct but `T`'s implementation of `Deserialize` decides that something
/// is wrong with the data, for example required struct fields are missing from
/// the YAML map or some number is too big to fit in the expected primitive
/// type.
pub fn from_value<T>(value: Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    Deserialize::deserialize(value)
}

impl Value {
    /// Index into a YAML sequence or map. A string index can be used to access
    /// a value in a map, and a usize index can be used to access an element of
    /// an sequence.
    ///
    /// Returns `None` if the type of `self` does not match the type of the
    /// index, for example if the index is a string and `self` is a sequence or
    /// a number. Also returns `None` if the given key does not exist in the map
    /// or the given index is not within the bounds of the sequence.
    ///
    /// Square brackets can also be used to index into a value in a more concise
    /// way. This returns `Value::Null` in cases where `get` would have returned
    /// `None`.
    pub fn get<I: Index>(&self, index: I) -> Option<&Value> {
        index.index_into(self)
    }

    /// Index into a YAML sequence or map. A string index can be used to access
    /// a value in a map, and a usize index can be used to access an element of
    /// an sequence.
    ///
    /// Returns `None` if the type of `self` does not match the type of the
    /// index, for example if the index is a string and `self` is a sequence or
    /// a number. Also returns `None` if the given key does not exist in the map
    /// or the given index is not within the bounds of the sequence.
    pub fn get_mut<I: Index>(&mut self, index: I) -> Option<&mut Value> {
        index.index_into_mut(self)
    }

    /// Returns true if the `Value` is a Null. Returns false otherwise.
    ///
    /// For any Value on which `is_null` returns true, `as_null` is guaranteed
    /// to return `Some(())`.
    pub fn is_null(&self) -> bool {
        matches!(self.untag_ref(), Value::Null)
    }

    /// If the `Value` is a Null, returns (). Returns None otherwise.
    pub fn as_null(&self) -> Option<()> {
        match self.untag_ref() {
            Value::Null => Some(()),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a Boolean. Returns false otherwise.
    ///
    /// For any Value on which `is_boolean` returns true, `as_bool` is
    /// guaranteed to return the boolean value.
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// If the `Value` is a Boolean, returns the associated bool. Returns None
    /// otherwise.
    pub fn as_bool(&self) -> Option<bool> {
        match self.untag_ref() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a Number. Returns false otherwise.
    pub fn is_number(&self) -> bool {
        matches!(self.untag_ref(), Value::Number(_))
    }

    /// Returns true if the `Value` is an integer between `i64::MIN` and
    /// `i64::MAX`.
    ///
    /// For any Value on which `is_i64` returns true, `as_i64` is guaranteed to
    /// return the integer value.
    pub fn is_i64(&self) -> bool {
        self.as_i64().is_some()
    }

    /// If the `Value` is an integer, represent it as i64 if possible. Returns
    /// None otherwise.
    pub fn as_i64(&self) -> Option<i64> {
        match self.untag_ref() {
            Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    /// Returns true if the `Value` is an integer between `u64::MIN` and
    /// `u64::MAX`.
    ///
    /// For any Value on which `is_u64` returns true, `as_u64` is guaranteed to
    /// return the integer value.
    pub fn is_u64(&self) -> bool {
        self.as_u64().is_some()
    }

    /// If the `Value` is an integer, represent it as u64 if possible. Returns
    /// None otherwise.
    pub fn as_u64(&self) -> Option<u64> {
        match self.untag_ref() {
            Value::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a number that can be represented by f64.
    ///
    /// For any Value on which `is_f64` returns true, `as_f64` is guaranteed to
    /// return the floating point value.
    ///
    /// Currently this function returns true if and only if both `is_i64` and
    /// `is_u64` return false but this is not a guarantee in the future.
    pub fn is_f64(&self) -> bool {
        match self.untag_ref() {
            Value::Number(n) => n.is_f64(),
            _ => false,
        }
    }

    /// If the `Value` is a number, represent it as f64 if possible. Returns
    /// None otherwise.
    pub fn as_f64(&self) -> Option<f64> {
        match self.untag_ref() {
            Value::Number(i) => i.as_f64(),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a String. Returns false otherwise.
    ///
    /// For any Value on which `is_string` returns true, `as_str` is guaranteed
    /// to return the string slice.
    pub fn is_string(&self) -> bool {
        self.as_str().is_some()
    }

    /// If the `Value` is a String, returns the associated str. Returns None
    /// otherwise.
    pub fn as_str(&self) -> Option<&str> {
        match self.untag_ref() {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a sequence. Returns false otherwise.
    pub fn is_sequence(&self) -> bool {
        self.as_sequence().is_some()
    }

    /// If the `Value` is a sequence, return a reference to it if possible.
    /// Returns None otherwise.
    pub fn as_sequence(&self) -> Option<&Sequence> {
        match self.untag_ref() {
            Value::Sequence(seq) => Some(seq),
            _ => None,
        }
    }

    /// If the `Value` is a sequence, return a mutable reference to it if
    /// possible. Returns None otherwise.
    pub fn as_sequence_mut(&mut self) -> Option<&mut Sequence> {
        match self.untag_mut() {
            Value::Sequence(seq) => Some(seq),
            _ => None,
        }
    }

    /// Returns true if the `Value` is a mapping. Returns false otherwise.
    pub fn is_mapping(&self) -> bool {
        self.as_mapping().is_some()
    }

    /// If the `Value` is a mapping, return a reference to it if possible.
    /// Returns None otherwise.
    pub fn as_mapping(&self) -> Option<&Mapping> {
        match self.untag_ref() {
            Value::Mapping(map) => Some(map),
            _ => None,
        }
    }

    /// If the `Value` is a mapping, return a reference to it if possible.
    /// Returns None otherwise.
    pub fn as_mapping_mut(&mut self) -> Option<&mut Mapping> {
        match self.untag_mut() {
            Value::Mapping(map) => Some(map),
            _ => None,
        }
    }

    /// Performs merging of `<<` keys into the surrounding mapping.
    ///
    /// The intended use of this in YAML is described in
    /// <https://yaml.org/type/merge.html>.
    pub fn apply_merge(&mut self) -> Result<(), Error> {
        let mut stack = Vec::new();
        stack.push(self);
        while let Some(node) = stack.pop() {
            match node {
                Value::Mapping(mapping) => {
                    match mapping.remove("<<") {
                        Some(Value::Mapping(merge)) => {
                            for (k, v) in merge {
                                mapping.entry(k).or_insert(v);
                            }
                        }
                        Some(Value::Sequence(sequence)) => {
                            for value in sequence {
                                match value {
                                    Value::Mapping(merge) => {
                                        for (k, v) in merge {
                                            mapping.entry(k).or_insert(v);
                                        }
                                    }
                                    Value::Sequence(_) => {
                                        return Err(error::new(ErrorImpl::SequenceInMergeElement));
                                    }
                                    Value::Tagged(_) => {
                                        return Err(error::new(ErrorImpl::TaggedInMerge));
                                    }
                                    _unexpected => {
                                        return Err(error::new(ErrorImpl::ScalarInMergeElement));
                                    }
                                }
                            }
                        }
                        None => {}
                        Some(Value::Tagged(_)) => return Err(error::new(ErrorImpl::TaggedInMerge)),
                        Some(_unexpected) => return Err(error::new(ErrorImpl::ScalarInMerge)),
                    }
                    stack.extend(mapping.values_mut());
                }
                Value::Sequence(sequence) => stack.extend(sequence),
                Value::Tagged(tagged) => stack.push(&mut tagged.value),
                _ => {}
            }
        }
        Ok(())
    }
}

impl Eq for Value {}

// NOTE: This impl must be kept consistent with HashLikeValue's Hash impl in
// mapping.rs in order for value[str] indexing to work.
impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        match self {
            Value::Null => {}
            Value::Bool(v) => v.hash(state),
            Value::Number(v) => v.hash(state),
            Value::String(v) => v.hash(state),
            Value::Sequence(v) => v.hash(state),
            Value::Mapping(v) => v.hash(state),
            Value::Tagged(v) => v.hash(state),
        }
    }
}

impl<'de> IntoDeserializer<'de, Error> for Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}
