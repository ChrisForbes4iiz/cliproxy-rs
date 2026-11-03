//! serde_yaml_ng 0.10's reading of an untagged plain scalar (its `de.rs`
//! `visit_untagged_scalar`), kept where cpa-core's text must not change with the
//! parser: mapping keys, which cpa-core reads as serde_yaml_ng did (see
//! [`super::load`]), the quoting [`super::to_string`] chooses, and [`super::Number`]'s
//! `FromStr`.

use std::num::ParseIntError;

use serde::de::Visitor;

use super::Error;

/// What serde_yaml_ng read a plain scalar as.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Plain {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    /// An integer past 64 bits that fits 128.
    Big,
    F64(f64),
    Str,
}

pub(crate) fn plain(v: &str) -> Plain {
    if v.is_empty() || parse_null(v.as_bytes()).is_some() {
        return Plain::Null;
    }
    if let Some(b) = parse_bool(v) {
        return Plain::Bool(b);
    }
    if let Some(n) = parse_unsigned_int(v, u64::from_str_radix) {
        return Plain::U64(n);
    }
    if let Some(n) = parse_negative_int(v, i64::from_str_radix) {
        return Plain::I64(n);
    }
    if parse_unsigned_int(v, u128::from_str_radix).is_some() || parse_negative_int(v, i128::from_str_radix).is_some() {
        return Plain::Big;
    }
    if !digits_but_not_number(v)
        && let Some(f) = parse_f64(v)
    {
        return Plain::F64(f);
    }
    Plain::Str
}

fn parse_null(scalar: &[u8]) -> Option<()> {
    matches!(scalar, b"null" | b"Null" | b"NULL" | b"~").then_some(())
}

fn parse_bool(scalar: &str) -> Option<bool> {
    match scalar {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

fn parse_unsigned_int<T>(scalar: &str, from_str_radix: fn(&str, radix: u32) -> Result<T, ParseIntError>) -> Option<T> {
    let unpositive = scalar.strip_prefix('+').unwrap_or(scalar);
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = unpositive.strip_prefix(prefix) {
            if rest.starts_with(['+', '-']) {
                return None;
            }
            if let Ok(int) = from_str_radix(rest, radix) {
                return Some(int);
            }
        }
    }
    if unpositive.starts_with(['+', '-']) {
        return None;
    }
    if digits_but_not_number(scalar) {
        return None;
    }
    from_str_radix(unpositive, 10).ok()
}

fn parse_negative_int<T>(scalar: &str, from_str_radix: fn(&str, radix: u32) -> Result<T, ParseIntError>) -> Option<T> {
    for (prefix, radix) in [("-0x", 16), ("-0o", 8), ("-0b", 2)] {
        if let Some(rest) = scalar.strip_prefix(prefix)
            && let Ok(int) = from_str_radix(&format!("-{rest}"), radix)
        {
            return Some(int);
        }
    }
    if digits_but_not_number(scalar) {
        return None;
    }
    from_str_radix(scalar, 10).ok()
}

pub(crate) fn parse_f64(scalar: &str) -> Option<f64> {
    let unpositive = if let Some(unpositive) = scalar.strip_prefix('+') {
        if unpositive.starts_with(['+', '-']) {
            return None;
        }
        unpositive
    } else {
        scalar
    };
    if let ".inf" | ".Inf" | ".INF" = unpositive {
        return Some(f64::INFINITY);
    }
    if let "-.inf" | "-.Inf" | "-.INF" = scalar {
        return Some(f64::NEG_INFINITY);
    }
    if let ".nan" | ".NaN" | ".NAN" = scalar {
        return Some(f64::NAN.copysign(1.0));
    }
    unpositive.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// Leading zeros followed by digits, which YAML 1.2 reads as a string.
pub(crate) fn digits_but_not_number(scalar: &str) -> bool {
    let scalar = scalar.strip_prefix(['-', '+']).unwrap_or(scalar);
    scalar.len() > 1 && scalar.starts_with('0') && scalar[1..].bytes().all(|b| b.is_ascii_digit())
}

/// serde_yaml_ng's `visit_int`: `Err(visitor)` when `v` is no integer.
pub(crate) fn visit_int<'de, V>(visitor: V, v: &str) -> Result<Result<V::Value, Error>, V>
where
    V: Visitor<'de>,
{
    if let Some(int) = parse_unsigned_int(v, u64::from_str_radix) {
        return Ok(visitor.visit_u64(int));
    }
    if let Some(int) = parse_negative_int(v, i64::from_str_radix) {
        return Ok(visitor.visit_i64(int));
    }
    if let Some(int) = parse_unsigned_int(v, u128::from_str_radix) {
        return Ok(visitor.visit_u128(int));
    }
    if let Some(int) = parse_negative_int(v, i128::from_str_radix) {
        return Ok(visitor.visit_i128(int));
    }
    Err(visitor)
}
