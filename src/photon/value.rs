//! Decoded Photon values.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use indexmap::IndexMap;

/// Parameter table keyed by Photon parameter byte.
pub type Params = BTreeMap<u8, Value>;

/// One deserialized Protocol18 value. Typed arrays keep their element type.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Byte(u8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    ByteArray(Vec<u8>),
    BoolArray(Vec<bool>),
    ShortArray(Vec<i16>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
    FloatArray(Vec<f32>),
    DoubleArray(Vec<f64>),
    StringArray(Vec<String>),
    Array(Vec<Value>),
    /// Dictionary or Hashtable, keys stringified (Go `%v` style).
    Hashtable(IndexMap<String, Value>),
    /// Nested OperationRequest / OperationResponse / EventData.
    Object(Vec<(&'static str, Value)>),
    Params(Params),
}

impl Value {
    /// Whether this value can be a dictionary key (Go: comparable types).
    pub(crate) fn is_hashable(&self) -> bool {
        matches!(
            self,
            Value::Null
                | Value::Bool(_)
                | Value::Byte(_)
                | Value::Short(_)
                | Value::Int(_)
                | Value::Long(_)
                | Value::Float(_)
                | Value::Double(_)
                | Value::String(_)
        )
    }

    /// Go `%T` name of the equivalent Go value, used for unhashable dictionary keys.
    pub(crate) fn go_type_name(&self) -> &'static str {
        match self {
            Value::Null => "<nil>",
            Value::Bool(_) => "bool",
            Value::Byte(_) => "uint8",
            Value::Short(_) => "int16",
            Value::Int(_) => "int32",
            Value::Long(_) => "int64",
            Value::Float(_) => "float32",
            Value::Double(_) => "float64",
            Value::String(_) => "string",
            Value::ByteArray(_) => "photon.ByteArray",
            Value::BoolArray(_) => "[]bool",
            Value::ShortArray(_) => "[]int16",
            Value::IntArray(_) => "[]int32",
            Value::LongArray(_) => "[]int64",
            Value::FloatArray(_) => "[]float32",
            Value::DoubleArray(_) => "[]float64",
            Value::StringArray(_) => "[]string",
            Value::Array(_) => "[]interface {}",
            Value::Hashtable(_) => "photon.Hashtable",
            Value::Object(_) => "map[string]interface {}",
            Value::Params(_) => "map[uint8]interface {}",
        }
    }

    /// Stringifies a hashable key like Go's `fmt.Sprintf("%v", key)`.
    pub(crate) fn key_string(&self) -> String {
        match self {
            Value::Null => "<nil>".to_owned(),
            Value::Bool(b) => b.to_string(),
            Value::Byte(v) => v.to_string(),
            Value::Short(v) => v.to_string(),
            Value::Int(v) => v.to_string(),
            Value::Long(v) => v.to_string(),
            Value::Float(v) => go_format_float(f64::from(*v), &format!("{v:e}")),
            Value::Double(v) => go_format_float(*v, &format!("{v:e}")),
            Value::String(s) => s.clone(),
            _ => String::new(),
        }
    }

    /// Integer view of a numeric value (used to read Albion codes out of params).
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Byte(v) => Some(i64::from(v)),
            Value::Short(v) => Some(i64::from(v)),
            Value::Int(v) => Some(i64::from(v)),
            Value::Long(v) => Some(v),
            _ => None,
        }
    }
}

/// Formats a float like Go's `%v`: shortest round-trip digits, switching to
/// exponent form when the exponent is < -4 or >= 6 (`1e+06`, `1e-05`).
/// `sci` is Rust's shortest `{:e}` rendering of the same value at its native width.
fn go_format_float(v: f64, sci: &str) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let (neg, rest) = match sci.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, sci),
    };
    let (mantissa, exp) = rest.split_once('e').unwrap_or((rest, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if digits == "0" {
        out.push('0');
        return out;
    }
    if !(-4..6).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let sign = if exp < 0 { '-' } else { '+' };
        let _ = write!(out, "e{sign}{:02}", exp.abs());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            for _ in digits.len()..int_len {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_keys_match_go_percent_v() {
        let cases: &[(Value, &str)] = &[
            (Value::Float(1e6), "1e+06"),
            (Value::Double(123_456_789.0), "1.23456789e+08"),
            (Value::Float(0.00001), "1e-05"),
            (Value::Float(1.5), "1.5"),
            (Value::Double(100_000.0), "100000"),
            (Value::Double(0.0001), "0.0001"),
            (Value::Float(-2.25), "-2.25"),
            (Value::Float(0.0), "0"),
            (Value::Null, "<nil>"),
            (Value::Bool(true), "true"),
            (Value::Short(-7), "-7"),
        ];
        for (v, want) in cases {
            assert_eq!(v.key_string(), *want, "{v:?}");
        }
    }
}
