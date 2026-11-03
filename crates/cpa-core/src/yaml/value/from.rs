use crate::yaml::{Mapping, Value};

// Implement a bunch of conversion to make it easier to create YAML values
// on the fly.

macro_rules! from_number {
    ($($ty:ident)*) => {
        $(
            impl From<$ty> for Value {
                fn from(n: $ty) -> Self {
                    Value::Number(n.into())
                }
            }
        )*
    };
}

from_number! {
    i8 i16 i32 i64 isize
    u8 u16 u32 u64 usize
    f32 f64
}

impl From<bool> for Value {
    /// Convert boolean to `Value`
    fn from(f: bool) -> Self {
        Value::Bool(f)
    }
}

impl From<String> for Value {
    /// Convert `String` to `Value`
    fn from(f: String) -> Self {
        Value::String(f)
    }
}

impl From<&str> for Value {
    /// Convert string slice to `Value`
    fn from(f: &str) -> Self {
        Value::String(f.to_string())
    }
}

use std::borrow::Cow;

impl<'a> From<Cow<'a, str>> for Value {
    /// Convert copy-on-write string to `Value`
    fn from(f: Cow<'a, str>) -> Self {
        Value::String(f.to_string())
    }
}

impl From<Mapping> for Value {
    /// Convert map (with string keys) to `Value`
    fn from(f: Mapping) -> Self {
        Value::Mapping(f)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    /// Convert a `Vec` to `Value`
    fn from(f: Vec<T>) -> Self {
        Value::Sequence(f.into_iter().map(Into::into).collect())
    }
}

impl<'a, T: Clone + Into<Value>> From<&'a [T]> for Value {
    /// Convert a slice to `Value`
    fn from(f: &'a [T]) -> Self {
        Value::Sequence(f.iter().cloned().map(Into::into).collect())
    }
}

impl<T: Into<Value>> FromIterator<T> for Value {
    /// Convert an iteratable type to a YAML sequence
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let vec = iter.into_iter().map(T::into).collect();

        Value::Sequence(vec)
    }
}
