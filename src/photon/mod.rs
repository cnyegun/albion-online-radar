//! Deserializes Albion Online's Photon Protocol18 traffic.
//!
//! [`PhotonParser`] turns UDP payloads into [`EventData`], [`OperationRequest`]
//! and [`OperationResponse`] values and hands them to a [`PhotonHandler`].
//!
//! Dispatch byte vs real code: `EventData::code` is the dispatch byte read from
//! the wire (typically 3 for Move, 1 for everything else). The gameplay-level
//! Albion event code is carried in `parameters[252]` (int16 for generic
//! events, or synthesized from `code` by [`post_process_event`] when absent).
//! Consumers should route on `parameters[252]`, not `code`. Same convention for
//! `parameters[253]` on operations.

mod deserializer;
#[cfg(test)]
mod live_tests;
mod packet;
mod reader;
mod typecodes;
mod value;

pub use packet::{PhotonHandler, PhotonParser};
pub use value::{Params, Value};


/// Albion event code for Move (dispatch byte 3).
pub const EVENT_CODE_MOVE: u8 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub code: u8,
    pub parameters: Params,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationRequest {
    pub operation_code: u8,
    pub parameters: Params,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationResponse {
    pub operation_code: u8,
    pub return_code: i16,
    pub debug_message: String,
    pub parameters: Params,
}

pub fn post_process_event(event: &mut EventData) {
    event.parameters.entry(252).or_insert(Value::Byte(event.code));
    if event.code == EVENT_CODE_MOVE {
        extract_move_positions(&mut event.parameters);
    }
}

pub fn post_process_request(req: &mut OperationRequest) {
    req.parameters.entry(253).or_insert(Value::Byte(req.operation_code));
}

pub fn post_process_response(resp: &mut OperationResponse) {
    resp.parameters.entry(253).or_insert(Value::Byte(resp.operation_code));
}

/// Mobs/resources send mode=3 with 30 bytes; players send mode=3 too but with
/// XOR-encrypted floats that decode to NaN/Inf without the XorCode. Those are
/// skipped so the front-end never sees garbage coordinates.
fn extract_move_positions(params: &mut Params) {
    let Some(Value::ByteArray(raw)) = params.get(&1) else { return };
    if raw.len() < 17 {
        return;
    }
    let x = f32::from_le_bytes([raw[9], raw[10], raw[11], raw[12]]);
    let y = f32::from_le_bytes([raw[13], raw[14], raw[15], raw[16]]);
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    params.insert(4, Value::Float(x));
    params.insert(5, Value::Float(y));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn move_event(raw: Vec<u8>) -> EventData {
        let mut ev = EventData { code: 3, parameters: Params::from([(1, Value::ByteArray(raw))]) };
        post_process_event(&mut ev);
        ev
    }

    fn pos(ev: &EventData, k: u8) -> f32 {
        match ev.parameters.get(&k) {
            Some(Value::Float(f)) => *f,
            other => panic!("param {k} = {other:?}"),
        }
    }

    #[test]
    fn move_live_samples() {
        let cases: [(&[u8], f32, f32); 3] = [
            (
                &[3, 104, 88, 142, 195, 84, 154, 222, 8, 176, 230, 36, 195, 237, 215, 175, 67, 166, 0, 0, 176, 63, 245, 26, 37, 195, 158, 196, 175, 67],
                -164.90112,
                351.68692,
            ),
            (
                &[3, 69, 127, 142, 195, 84, 154, 222, 8, 39, 231, 225, 194, 19, 187, 160, 67, 164, 0, 0, 64, 64, 148, 73, 233, 194, 164, 52, 159, 67],
                -112.95147,
                321.46152,
            ),
            (
                &[3, 218, 111, 173, 195, 84, 154, 222, 8, 131, 204, 183, 194, 26, 170, 171, 67, 29, 0, 0, 128, 63, 92, 82, 178, 194, 166, 68, 173, 67],
                -91.89944,
                343.32892,
            ),
        ];
        for (raw, x, y) in cases {
            let ev = move_event(raw.to_vec());
            assert!((pos(&ev, 4) - x).abs() < 0.001);
            assert!((pos(&ev, 5) - y).abs() < 0.001);
        }
    }

    #[test]
    fn move_skips_nan_and_inf() {
        for bits in [[0x7fc00000u32, 0x7fc00000], [0x7fc00000, 12.5f32.to_bits()], [12.5f32.to_bits(), 0xff800000]] {
            let mut raw = vec![0u8; 17];
            raw[9..13].copy_from_slice(&bits[0].to_le_bytes());
            raw[13..17].copy_from_slice(&bits[1].to_le_bytes());
            let ev = move_event(raw);
            assert!(!ev.parameters.contains_key(&4) && !ev.parameters.contains_key(&5));
        }
    }

    #[test]
    fn move_short_array_is_noop() {
        let ev = move_event(vec![0, 0, 0]);
        assert!(!ev.parameters.contains_key(&4));
    }

    #[test]
    fn code_fallbacks() {
        let mut ev = EventData { code: 29, parameters: Params::new() };
        post_process_event(&mut ev);
        assert_eq!(ev.parameters[&252], Value::Byte(29));

        let mut ev = EventData { code: 29, parameters: Params::from([(252, Value::Byte(99))]) };
        post_process_event(&mut ev);
        assert_eq!(ev.parameters[&252], Value::Byte(99));

        let mut req = OperationRequest { operation_code: 21, parameters: Params::new() };
        post_process_request(&mut req);
        assert_eq!(req.parameters[&253], Value::Byte(21));

        let mut resp = OperationResponse { operation_code: 42, return_code: 0, debug_message: String::new(), parameters: Params::new() };
        post_process_response(&mut resp);
        assert_eq!(resp.parameters[&253], Value::Byte(42));
    }
}
