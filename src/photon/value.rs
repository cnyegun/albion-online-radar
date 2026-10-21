//! Decoded Photon values.

use std::collections::BTreeMap;

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
    /// Dictionary or Hashtable, keys stringified.
    Hashtable(IndexMap<String, Value>),
    /// Nested OperationRequest / OperationResponse / EventData.
    Object(Vec<(&'static str, Value)>),
    Params(Params),
}

impl Value {
    /// Dictionary key text for scalar keys; `None` for arrays and tables.
    pub(crate) fn key_string(&self) -> Option<String> {
        Some(match self {
            Value::Null => "null".to_owned(),
            Value::Bool(b) => b.to_string(),
            Value::Byte(v) => v.to_string(),
            Value::Short(v) => v.to_string(),
            Value::Int(v) => v.to_string(),
            Value::Long(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Double(v) => v.to_string(),
            Value::String(s) => s.clone(),
            _ => return None,
        })
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
