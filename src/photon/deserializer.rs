//! Protocol18 value deserializer.

use indexmap::IndexMap;

use super::reader::Reader;
use super::typecodes::*;
use super::value::{Params, Value};
use super::{EventData, OperationRequest, OperationResponse};

/// Nesting cap for containers. Real traffic nests a handful of levels; the cap
/// keeps a hostile payload from recursing until the stack overflows.
const MAX_DEPTH: u32 = 64;

pub(crate) fn deserialize(r: &mut Reader, tc: u8) -> Value {
    deserialize_at(r, tc, 0)
}

fn deserialize_at(r: &mut Reader, tc: u8, depth: u32) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    let depth = depth + 1;
    if tc >= CUSTOM_TYPE_SLIM_BASE {
        return deserialize_custom(r, tc);
    }
    match tc {
        TYPE_UNKNOWN | TYPE_NULL => Value::Null,
        TYPE_BOOLEAN => Value::Bool(r.read_byte().unwrap_or(0) != 0),
        TYPE_BYTE => Value::Byte(r.read_byte().unwrap_or(0)),
        TYPE_SHORT => Value::Short(r.i16()),
        TYPE_FLOAT => Value::Float(r.f32()),
        TYPE_DOUBLE => Value::Double(r.f64()),
        TYPE_STRING => Value::String(r.string()),
        TYPE_COMPRESSED_INT => Value::Int(r.compressed_i32()),
        TYPE_COMPRESSED_LONG => Value::Long(r.compressed_i64()),
        TYPE_INT1 => Value::Int(i32::from(r.read_byte().unwrap_or(0))),
        TYPE_INT1_NEG => Value::Int(-i32::from(r.read_byte().unwrap_or(0))),
        TYPE_INT2 => Value::Int(i32::from(r.u16())),
        TYPE_INT2_NEG => Value::Int(-i32::from(r.u16())),
        TYPE_LONG1 => Value::Long(i64::from(r.read_byte().unwrap_or(0))),
        TYPE_LONG1_NEG => Value::Long(-i64::from(r.read_byte().unwrap_or(0))),
        TYPE_LONG2 => Value::Long(i64::from(r.u16())),
        TYPE_LONG2_NEG => Value::Long(-i64::from(r.u16())),
        TYPE_CUSTOM => deserialize_custom(r, 0),
        TYPE_DICTIONARY | TYPE_HASHTABLE => Value::Hashtable(deserialize_dictionary(r, depth)),
        TYPE_OBJECT_ARRAY => deserialize_object_array(r, depth),
        TYPE_OPERATION_REQUEST => {
            let op_code = r.read_byte().unwrap_or(0);
            let params = read_parameter_table_at(r, depth);
            Value::Object(vec![
                ("operationCode", Value::Byte(op_code)),
                ("parameters", Value::Params(params)),
            ])
        }
        TYPE_OPERATION_RESPONSE => {
            if r.len() < 3 {
                return Value::Null;
            }
            let op_code = r.read_byte().unwrap_or(0);
            let return_code = r.i16();
            let mut debug = String::new();
            if let Some(tc) = r.read_byte()
                && let Value::String(s) = deserialize_at(r, tc, depth)
            {
                debug = s;
            }
            let params = read_parameter_table_at(r, depth);
            Value::Object(vec![
                ("operationCode", Value::Byte(op_code)),
                ("returnCode", Value::Short(return_code)),
                ("debugMessage", Value::String(debug)),
                ("parameters", Value::Params(params)),
            ])
        }
        TYPE_EVENT_DATA => {
            let code = r.read_byte().unwrap_or(0);
            let params = read_parameter_table_at(r, depth);
            Value::Object(vec![("code", Value::Byte(code)), ("parameters", Value::Params(params))])
        }
        TYPE_BOOL_FALSE => Value::Bool(false),
        TYPE_BOOL_TRUE => Value::Bool(true),
        TYPE_SHORT_ZERO => Value::Short(0),
        TYPE_INT_ZERO => Value::Int(0),
        TYPE_LONG_ZERO => Value::Long(0),
        TYPE_FLOAT_ZERO => Value::Float(0.0),
        TYPE_DOUBLE_ZERO => Value::Double(0.0),
        TYPE_BYTE_ZERO => Value::Byte(0),
        TYPE_ARRAY => deserialize_nested_array(r, depth),
        _ if tc & TYPE_ARRAY == TYPE_ARRAY => deserialize_typed_array(r, tc & !TYPE_ARRAY, depth),
        _ => Value::Null,
    }
}

fn deserialize_custom(r: &mut Reader, gp_type: u8) -> Value {
    if gp_type < CUSTOM_TYPE_SLIM_BASE && r.read_byte().is_none() {
        return Value::Null;
    }
    let size = r.count();
    if size > r.len() || size > MAX_ARRAY_SIZE {
        return Value::Null;
    }
    Value::ByteArray(r.next(size).to_vec())
}

fn deserialize_dictionary(r: &mut Reader, depth: u32) -> IndexMap<String, Value> {
    let Some(key_tc) = r.read_byte() else { return IndexMap::new() };
    let Some(val_tc) = r.read_byte() else { return IndexMap::new() };
    let count = r.count();
    if count > MAX_ARRAY_SIZE || count > r.len() {
        return IndexMap::new();
    }
    let mut out = IndexMap::with_capacity(count);
    for i in 0..count {
        if r.is_empty() {
            break;
        }
        let kt = if key_tc == 0 {
            match r.read_byte() {
                Some(t) => t,
                None => return out,
            }
        } else {
            key_tc
        };
        let vt = if val_tc == 0 {
            match r.read_byte() {
                Some(t) => t,
                None => return out,
            }
        } else {
            val_tc
        };
        let key = deserialize_at(r, kt, depth);
        let val = deserialize_at(r, vt, depth);
        let key = if key.is_hashable() {
            key.key_string()
        } else {
            format!("UNHASHABLE_{i}_{}", key.go_type_name())
        };
        out.insert(key, val);
    }
    out
}

fn deserialize_object_array(r: &mut Reader, depth: u32) -> Value {
    let size = r.count();
    if size > MAX_ARRAY_SIZE || size > r.len() {
        return Value::Null;
    }
    let mut out = vec![Value::Null; size];
    for slot in &mut out {
        let Some(tc) = r.read_byte() else { break };
        *slot = deserialize_at(r, tc, depth);
    }
    Value::Array(out)
}

fn deserialize_nested_array(r: &mut Reader, depth: u32) -> Value {
    let size = r.count();
    if size > MAX_ARRAY_SIZE || size > r.len() {
        return Value::Null;
    }
    let Some(tc) = r.read_byte() else { return Value::Null };
    Value::Array((0..size).map(|_| deserialize_at(r, tc, depth)).collect())
}

fn deserialize_typed_array(r: &mut Reader, elem: u8, depth: u32) -> Value {
    let size = r.count();
    if size > MAX_ARRAY_SIZE {
        return Value::Null;
    }
    match elem {
        TYPE_BOOLEAN => {
            let mut packed = vec![0u8; size.div_ceil(8)];
            r.read_into(&mut packed);
            Value::BoolArray((0..size).map(|i| packed[i / 8] & (1 << (i % 8)) != 0).collect())
        }
        TYPE_BYTE => {
            let mut data = vec![0u8; size];
            r.read_into(&mut data);
            Value::ByteArray(data)
        }
        TYPE_SHORT => Value::ShortArray((0..size).map(|_| r.i16()).collect()),
        TYPE_FLOAT => Value::FloatArray((0..size).map(|_| r.f32()).collect()),
        TYPE_DOUBLE => Value::DoubleArray((0..size).map(|_| r.f64()).collect()),
        TYPE_STRING => Value::StringArray((0..size).map(|_| r.string()).collect()),
        TYPE_COMPRESSED_INT => Value::IntArray((0..size).map(|_| r.compressed_i32()).collect()),
        TYPE_COMPRESSED_LONG => Value::LongArray((0..size).map(|_| r.compressed_i64()).collect()),
        TYPE_DICTIONARY | TYPE_HASHTABLE => {
            Value::Array((0..size).map(|_| Value::Hashtable(deserialize_dictionary(r, depth))).collect())
        }
        TYPE_CUSTOM => {
            // Shared custom type id for all elements, then N sized payloads.
            if r.read_byte().is_none() {
                return Value::Null;
            }
            let mut out = Vec::with_capacity(size.min(r.len()));
            for _ in 0..size {
                let elem_size = r.count();
                if elem_size > r.len() || elem_size > MAX_ARRAY_SIZE {
                    return Value::Null;
                }
                out.push(Value::ByteArray(r.next(elem_size).to_vec()));
            }
            Value::Array(out)
        }
        _ => Value::Array((0..size).map(|_| deserialize_at(r, elem, depth)).collect()),
    }
}

pub(crate) fn read_parameter_table(r: &mut Reader) -> Params {
    read_parameter_table_at(r, 0)
}

fn read_parameter_table_at(r: &mut Reader, depth: u32) -> Params {
    let mut params = Params::new();
    let count = r.count();
    if count > MAX_ARRAY_SIZE || count > r.len() {
        return params;
    }
    for _ in 0..count {
        if r.is_empty() {
            break;
        }
        let Some(key) = r.read_byte() else { break };
        let Some(tc) = r.read_byte() else { break };
        params.insert(key, deserialize_at(r, tc, depth));
    }
    params
}

pub fn deserialize_event(data: &[u8]) -> Option<EventData> {
    let mut r = Reader::new(data);
    let code = r.read_byte()?;
    let parameters = read_parameter_table(&mut r);
    Some(EventData { code, parameters })
}

pub fn deserialize_request(data: &[u8]) -> Option<OperationRequest> {
    let mut r = Reader::new(data);
    let operation_code = r.read_byte()?;
    let parameters = read_parameter_table(&mut r);
    Some(OperationRequest { operation_code, parameters })
}

pub fn deserialize_response(data: &[u8]) -> Option<OperationResponse> {
    if data.len() < 3 {
        return None;
    }
    let mut r = Reader::new(data);
    let operation_code = r.read_byte()?;
    let return_code = r.i16();

    // Protocol18 reserves a typed slot here for the debug message. Albion
    // hijacks it on market-order responses to carry the order payload as a
    // string array, expected by downstream handlers at params[0].
    let mut debug_message = String::new();
    let mut market_orders = None;
    if let Some(tc) = r.read_byte() {
        match deserialize(&mut r, tc) {
            Value::String(s) => debug_message = s,
            Value::StringArray(v) => market_orders = Some(v),
            _ => {}
        }
    }

    let mut parameters = read_parameter_table(&mut r);
    if let Some(orders) = market_orders {
        parameters.insert(0, Value::StringArray(orders));
    }
    Some(OperationResponse { operation_code, return_code, debug_message, parameters })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(tc: u8, payload: &[u8]) -> Value {
        deserialize(&mut Reader::new(payload), tc)
    }

    fn ht(pairs: &[(&str, Value)]) -> Value {
        Value::Hashtable(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
    }

    #[test]
    fn primitives() {
        let cases: Vec<(u8, &[u8], Value)> = vec![
            (TYPE_NULL, &[], Value::Null),
            (TYPE_UNKNOWN, &[], Value::Null),
            (TYPE_BOOLEAN, &[0x01], Value::Bool(true)),
            (TYPE_BOOLEAN, &[0x00], Value::Bool(false)),
            (TYPE_BYTE, &[0x2a], Value::Byte(0x2a)),
            (TYPE_SHORT, &[0x34, 0x12], Value::Short(0x1234)),
            (TYPE_FLOAT, &[0x00, 0x00, 0x80, 0x3f], Value::Float(1.0)),
            (TYPE_DOUBLE, &[0, 0, 0, 0, 0, 0, 0xf0, 0x3f], Value::Double(1.0)),
            (TYPE_STRING, &[0x03, b'f', b'o', b'o'], Value::String("foo".into())),
            (TYPE_COMPRESSED_INT, &[0x04], Value::Int(2)),
            (TYPE_COMPRESSED_LONG, &[0x02], Value::Long(1)),
            (TYPE_INT1, &[0x05], Value::Int(5)),
            (TYPE_INT1_NEG, &[0x05], Value::Int(-5)),
            (TYPE_INT2, &[0x34, 0x12], Value::Int(0x1234)),
            (TYPE_INT2_NEG, &[0x34, 0x12], Value::Int(-0x1234)),
            (TYPE_LONG1, &[0x05], Value::Long(5)),
            (TYPE_LONG1_NEG, &[0x05], Value::Long(-5)),
            (TYPE_LONG2, &[0x34, 0x12], Value::Long(0x1234)),
            (TYPE_LONG2_NEG, &[0x34, 0x12], Value::Long(-0x1234)),
            (TYPE_BOOL_FALSE, &[], Value::Bool(false)),
            (TYPE_BOOL_TRUE, &[], Value::Bool(true)),
            (TYPE_SHORT_ZERO, &[], Value::Short(0)),
            (TYPE_INT_ZERO, &[], Value::Int(0)),
            (TYPE_LONG_ZERO, &[], Value::Long(0)),
            (TYPE_FLOAT_ZERO, &[], Value::Float(0.0)),
            (TYPE_DOUBLE_ZERO, &[], Value::Double(0.0)),
            (TYPE_BYTE_ZERO, &[], Value::Byte(0)),
        ];
        for (tc, payload, want) in cases {
            assert_eq!(de(tc, payload), want, "tc={tc}");
        }
    }

    #[test]
    fn custom() {
        assert_eq!(de(TYPE_CUSTOM, &[42, 0x03, 1, 2, 3]), Value::ByteArray(vec![1, 2, 3]));
        assert_eq!(de(0x85, &[0x02, 0xaa, 0xbb]), Value::ByteArray(vec![0xaa, 0xbb]));
        assert_eq!(de(TYPE_CUSTOM, &[42, 0x0a, 1, 2]), Value::Null);
    }

    #[test]
    fn arrays() {
        assert_eq!(
            de(TYPE_OBJECT_ARRAY, &[0x02, TYPE_BYTE, 0x01, TYPE_SHORT, 0x34, 0x12]),
            Value::Array(vec![Value::Byte(1), Value::Short(0x1234)])
        );
        assert_eq!(
            de(TYPE_ARRAY, &[0x03, TYPE_BYTE, 0x0a, 0x0b, 0x0c]),
            Value::Array(vec![Value::Byte(0x0a), Value::Byte(0x0b), Value::Byte(0x0c)])
        );
        assert_eq!(de(TYPE_ARRAY | TYPE_BYTE, &[0x03, 0x0a, 0x0b, 0x0c]), Value::ByteArray(vec![0x0a, 0x0b, 0x0c]));
        assert_eq!(de(TYPE_ARRAY | TYPE_SHORT, &[0x02, 0x34, 0x12, 0x78, 0x56]), Value::ShortArray(vec![0x1234, 0x5678]));
        assert_eq!(de(TYPE_ARRAY | TYPE_FLOAT, &[0x01, 0x00, 0x00, 0x80, 0x3f]), Value::FloatArray(vec![1.0]));
        assert_eq!(
            de(TYPE_ARRAY | TYPE_STRING, &[0x02, 0x01, b'a', 0x02, b'b', b'c']),
            Value::StringArray(vec!["a".into(), "bc".into()])
        );
        assert_eq!(
            de(TYPE_ARRAY | TYPE_BOOLEAN, &[0x0a, 0xaa, 0x03]),
            Value::BoolArray(vec![false, true, false, true, false, true, false, true, true, true])
        );
        assert_eq!(
            de(TYPE_ARRAY | TYPE_CUSTOM, &[0x02, 0x07, 0x02, 0xaa, 0xbb, 0x01, 0xcc]),
            Value::Array(vec![Value::ByteArray(vec![0xaa, 0xbb]), Value::ByteArray(vec![0xcc])])
        );
        assert_eq!(de(TYPE_ARRAY | TYPE_BYTE, &[0xff, 0xff, 0xff, 0xff, 0x0f]), Value::Null);
    }

    #[test]
    fn dictionaries() {
        assert_eq!(
            de(TYPE_DICTIONARY, &[TYPE_BYTE, TYPE_SHORT, 0x02, 0x01, 0x34, 0x12, 0x02, 0x78, 0x56]),
            ht(&[("1", Value::Short(0x1234)), ("2", Value::Short(0x5678))])
        );
        assert_eq!(
            de(TYPE_DICTIONARY, &[0x00, 0x00, 0x01, TYPE_BYTE, TYPE_STRING, 0x2a, 0x02, b'h', b'i']),
            ht(&[("42", Value::String("hi".into()))])
        );
        assert_eq!(de(TYPE_HASHTABLE, &[TYPE_BYTE, TYPE_BYTE, 0x01, 0x07, 0x08]), ht(&[("7", Value::Byte(8))]));
        assert_eq!(
            de(TYPE_DICTIONARY, &[TYPE_ARRAY | TYPE_BYTE, TYPE_BYTE, 0x01, 0x01, 0x09, 0x05]),
            ht(&[("UNHASHABLE_0_photon.ByteArray", Value::Byte(5))])
        );
    }

    #[test]
    fn nested_op_request_does_not_desync() {
        let payload = [TYPE_BYTE, TYPE_OPERATION_REQUEST, 0x01, 0x01, 0x07, 0x00, 0xAA];
        let mut r = Reader::new(&payload);
        let Value::Hashtable(h) = deserialize(&mut r, TYPE_DICTIONARY) else { panic!() };
        assert_eq!(h.len(), 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r.read_byte(), Some(0xAA));
    }

    #[test]
    fn parameter_tables() {
        let got = read_parameter_table(&mut Reader::new(&[0x02, 0x00, TYPE_BYTE, 0x2a, 0xfc, TYPE_BYTE, 0x03]));
        assert_eq!(got[&0], Value::Byte(0x2a));
        assert_eq!(got[&252], Value::Byte(3));

        let got = read_parameter_table(&mut Reader::new(&[0x02, 0x00, TYPE_BYTE, 0x2a]));
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn deep_nesting_is_capped_not_fatal() {
        // 100k nested object arrays of size 1 would blow the stack without a cap.
        let mut payload = Vec::new();
        for _ in 0..100_000 {
            payload.extend_from_slice(&[0x01, TYPE_OBJECT_ARRAY]);
        }
        let _ = deserialize(&mut Reader::new(&payload), TYPE_OBJECT_ARRAY);
    }

    #[test]
    fn response_with_debug_string_and_market_orders() {
        let resp = deserialize_response(&[0x0f, 0x00, 0x00, TYPE_STRING, 0x02, b'o', b'k', 0x00]).unwrap();
        assert_eq!(resp.operation_code, 15);
        assert_eq!(resp.return_code, 0);
        assert_eq!(resp.debug_message, "ok");

        let resp = deserialize_response(&[0x15, 0x00, 0x00, TYPE_ARRAY | TYPE_STRING, 0x01, 0x03, b'a', b'b', b'c']).unwrap();
        assert!(resp.debug_message.is_empty());
        assert_eq!(resp.parameters[&0], Value::StringArray(vec!["abc".into()]));
    }

    #[test]
    fn request_op_move_post_patch() {
        let payload = [
            0x16, 0x05, 0x00, TYPE_LONG2, 0xe8, 0x03, 0x01, TYPE_ARRAY | TYPE_FLOAT, 0x02, 0x00, 0x00, 0x28, 0x41, 0x00,
            0x00, 0xa4, 0x41, 0x02, TYPE_FLOAT, 0x00, 0x00, 0xc0, 0x3f, 0x03, TYPE_ARRAY | TYPE_FLOAT, 0x02, 0x00, 0x00,
            0x18, 0x41, 0x00, 0x00, 0x9c, 0x41, 0x04, TYPE_FLOAT, 0x00, 0x00, 0xb0, 0x40,
        ];
        let req = deserialize_request(&payload).unwrap();
        assert_eq!(req.operation_code, 22);
        assert_eq!(req.parameters[&0], Value::Long(1000));
        assert_eq!(req.parameters[&1], Value::FloatArray(vec![10.5, 20.5]));
        assert_eq!(req.parameters[&2], Value::Float(1.5));
        assert_eq!(req.parameters[&3], Value::FloatArray(vec![9.5, 19.5]));
        assert_eq!(req.parameters[&4], Value::Float(5.5));
    }

    #[test]
    fn join_response_hashtable_at_param_103() {
        let payload = [
            0x02, 0x00, 0x00, TYPE_NULL, 0x02, 0x09, TYPE_ARRAY | TYPE_FLOAT, 0x02, 0x00, 0x00, 0xc8, 0x42, 0x00, 0x00, 0x48,
            0x43, 0x67, TYPE_HASHTABLE, TYPE_BYTE, TYPE_BYTE, 0x02, 0x05, 0x01, 0x07, 0x00,
        ];
        let resp = deserialize_response(&payload).unwrap();
        assert_eq!(resp.parameters[&9], Value::FloatArray(vec![100.0, 200.0]));
        assert_eq!(resp.parameters[&103], ht(&[("5", Value::Byte(1)), ("7", Value::Byte(0))]));
    }
}
