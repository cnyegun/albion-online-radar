//! Photon UDP packet framing: commands, reliable/unreliable messages and
//! fragment reassembly.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::deserializer::{deserialize_event, deserialize_request, deserialize_response};
use super::typecodes::MAX_ARRAY_SIZE;
use super::{EventData, OperationRequest, OperationResponse};

const PHOTON_HEADER_LENGTH: usize = 12;
const COMMAND_HEADER_LENGTH: usize = 12;
const FRAGMENT_HEADER_LENGTH: usize = 20;

/// Caps reassembly memory at ~64 x 1 MB per parser under packet loss.
const MAX_PENDING_SEGMENTS: usize = 64;

const CMD_DISCONNECT: u8 = 4;
const CMD_SEND_RELIABLE: u8 = 6;
const CMD_SEND_UNRELIABLE: u8 = 7;
const CMD_SEND_FRAGMENT: u8 = 8;

const MSG_REQUEST: u8 = 2;
const MSG_RESPONSE: u8 = 3;
const MSG_EVENT: u8 = 4;
const MSG_RESPONSE_ALT: u8 = 7;
const MSG_ENCRYPTED: u8 = 131;

/// Receives everything the parser decodes out of a packet.
pub trait PhotonHandler {
    fn on_event(&mut self, _event: EventData) {}
    fn on_request(&mut self, _request: OperationRequest) {}
    fn on_response(&mut self, _response: OperationResponse) {}
    fn on_encrypted(&mut self) {}
    fn on_parse_error(&mut self, _reason: &str, _payload_len: usize) {}
}

struct SegmentedPackage {
    total_length: usize,
    bytes_written: usize,
    payload: Vec<u8>,
    created_at: Instant,
    /// Deduplicates repeated fragments under UDP loss/retransmit. Without it a
    /// duplicate would bump `bytes_written` twice and complete the message
    /// early with a partially zeroed payload.
    seen_offsets: HashSet<usize>,
}

#[derive(Default)]
pub struct PhotonParser {
    pending_segments: HashMap<u32, SegmentedPackage>,
}

fn be_u32(src: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([src[offset], src[offset + 1], src[offset + 2], src[offset + 3]])
}

fn available(src: &[u8], offset: usize, count: usize) -> bool {
    offset <= src.len() && src.len() - offset >= count
}

impl PhotonParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one UDP payload. Returns false when the packet was rejected
    /// (too short, encrypted header flag, or a malformed command).
    pub fn receive_packet(&mut self, payload: &[u8], h: &mut impl PhotonHandler) -> bool {
        if payload.len() < PHOTON_HEADER_LENGTH {
            h.on_parse_error("payload shorter than photon header", payload.len());
            return false;
        }
        let flags = payload[2];
        let command_count = payload[3];
        let mut offset = PHOTON_HEADER_LENGTH; // peerId, flags, count, timestamp, challenge

        if flags == 1 {
            h.on_encrypted();
            return false;
        }

        for _ in 0..command_count {
            match self.handle_command(payload, offset, h) {
                Some(next) => offset = next,
                None => {
                    h.on_parse_error("handleCommand failed", payload.len());
                    return false;
                }
            }
        }
        true
    }

    fn handle_command(&mut self, src: &[u8], mut offset: usize, h: &mut impl PhotonHandler) -> Option<usize> {
        if !available(src, offset, COMMAND_HEADER_LENGTH) {
            return None;
        }
        let cmd_type = src[offset];
        offset += 4; // cmdType, channelId, commandFlags, reserved
        let cmd_len = (be_u32(src, offset) as usize).checked_sub(COMMAND_HEADER_LENGTH)?;
        offset += 8; // length + reliableSequenceNumber
        if !available(src, offset, cmd_len) {
            return None;
        }

        match cmd_type {
            CMD_DISCONNECT => Some(offset + cmd_len),
            CMD_SEND_UNRELIABLE => {
                if cmd_len < 4 {
                    return None;
                }
                Some(self.handle_send_reliable(src, offset + 4, cmd_len - 4, h))
            }
            CMD_SEND_RELIABLE => Some(self.handle_send_reliable(src, offset, cmd_len, h)),
            CMD_SEND_FRAGMENT => Some(self.handle_send_fragment(src, offset, cmd_len, h)),
            _ => Some(offset + cmd_len),
        }
    }

    fn handle_send_reliable(&mut self, src: &[u8], offset: usize, cmd_len: usize, h: &mut impl PhotonHandler) -> usize {
        if cmd_len < 2 || !available(src, offset, cmd_len) {
            return offset + cmd_len;
        }
        let msg_type = src[offset + 1]; // after signalByte
        let start = offset + 2;
        let end = offset + cmd_len;

        if msg_type == MSG_ENCRYPTED {
            h.on_encrypted();
            return end;
        }

        let data = &src[start..end];
        match msg_type {
            MSG_REQUEST => {
                if let Some(req) = deserialize_request(data) {
                    h.on_request(req);
                }
            }
            MSG_RESPONSE | MSG_RESPONSE_ALT => {
                if let Some(resp) = deserialize_response(data) {
                    h.on_response(resp);
                }
            }
            MSG_EVENT => {
                if let Some(ev) = deserialize_event(data) {
                    h.on_event(ev);
                }
            }
            _ => {}
        }
        end
    }

    fn handle_send_fragment(&mut self, src: &[u8], offset: usize, cmd_len: usize, h: &mut impl PhotonHandler) -> usize {
        if cmd_len < FRAGMENT_HEADER_LENGTH || !available(src, offset, FRAGMENT_HEADER_LENGTH) {
            return offset + cmd_len;
        }

        let start_seq = be_u32(src, offset);
        // offset + 4: fragmentCount, offset + 8: fragmentNumber
        let total_len = be_u32(src, offset + 12) as usize;
        let frag_offset = be_u32(src, offset + 16) as usize;
        let data_start = offset + FRAGMENT_HEADER_LENGTH;
        let frag_len = cmd_len - FRAGMENT_HEADER_LENGTH;

        if !available(src, data_start, frag_len) || total_len > MAX_ARRAY_SIZE * 16 {
            return data_start + frag_len;
        }

        if !self.pending_segments.contains_key(&start_seq) {
            self.evict_if_full();
            self.pending_segments.insert(
                start_seq,
                SegmentedPackage {
                    total_length: total_len,
                    bytes_written: 0,
                    payload: vec![0; total_len],
                    created_at: Instant::now(),
                    seen_offsets: HashSet::new(),
                },
            );
        }
        let Some(seg) = self.pending_segments.get_mut(&start_seq) else {
            return data_start + frag_len;
        };

        let end = frag_offset.saturating_add(frag_len);
        if !seg.seen_offsets.contains(&frag_offset) && end <= seg.payload.len() {
            seg.payload[frag_offset..end].copy_from_slice(&src[data_start..data_start + frag_len]);
            seg.bytes_written += frag_len;
            seg.seen_offsets.insert(frag_offset);
        }

        if seg.bytes_written >= seg.total_length
            && let Some(done) = self.pending_segments.remove(&start_seq)
        {
            let len = done.payload.len();
            self.handle_send_reliable(&done.payload, 0, len, h);
        }
        data_start + frag_len
    }

    fn evict_if_full(&mut self) {
        if self.pending_segments.len() < MAX_PENDING_SEGMENTS {
            return;
        }
        if let Some(oldest) = self.pending_segments.iter().min_by_key(|(_, seg)| seg.created_at).map(|(k, _)| *k) {
            self.pending_segments.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending_segments.len()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::photon::Value;
    use crate::photon::typecodes::*;

    #[derive(Default)]
    pub(crate) struct Collect {
        pub events: Vec<EventData>,
        pub requests: Vec<OperationRequest>,
        pub responses: Vec<OperationResponse>,
        pub encrypted: usize,
        pub errors: Vec<(String, usize)>,
    }

    impl PhotonHandler for Collect {
        fn on_event(&mut self, e: EventData) {
            self.events.push(e);
        }
        fn on_request(&mut self, r: OperationRequest) {
            self.requests.push(r);
        }
        fn on_response(&mut self, r: OperationResponse) {
            self.responses.push(r);
        }
        fn on_encrypted(&mut self) {
            self.encrypted += 1;
        }
        fn on_parse_error(&mut self, reason: &str, len: usize) {
            self.errors.push((reason.to_owned(), len));
        }
    }

    fn single_command_packet(cmd_type: u8, cmd_payload: &[u8]) -> Vec<u8> {
        let cmd_len = COMMAND_HEADER_LENGTH + cmd_payload.len();
        let mut pkt = vec![0u8; PHOTON_HEADER_LENGTH + cmd_len];
        pkt[3] = 1;
        pkt[PHOTON_HEADER_LENGTH] = cmd_type;
        pkt[PHOTON_HEADER_LENGTH + 4..PHOTON_HEADER_LENGTH + 8].copy_from_slice(&(cmd_len as u32).to_be_bytes());
        pkt[PHOTON_HEADER_LENGTH + COMMAND_HEADER_LENGTH..].copy_from_slice(cmd_payload);
        pkt
    }

    fn reliable_message_packet(msg_type: u8, inner: &[u8]) -> Vec<u8> {
        let mut reliable = vec![0, msg_type];
        reliable.extend_from_slice(inner);
        single_command_packet(CMD_SEND_RELIABLE, &reliable)
    }

    const EVENT_PAYLOAD: [u8; 5] = [0x03, 0x01, 0xfc, TYPE_BYTE, 0x03];

    fn fragmented_event_packets(n: usize) -> Vec<Vec<u8>> {
        let mut reliable = vec![0x00, MSG_EVENT];
        reliable.extend_from_slice(&EVENT_PAYLOAD);
        let total = reliable.len();
        let chunk = total.div_ceil(n);
        (0..n)
            .map(|i| {
                let start = i * chunk;
                let end = (start + chunk).min(total);
                let mut p = Vec::new();
                for v in [100, n as u32, i as u32, total as u32, start as u32] {
                    p.extend_from_slice(&v.to_be_bytes());
                }
                p.extend_from_slice(&reliable[start..end]);
                single_command_packet(CMD_SEND_FRAGMENT, &p)
            })
            .collect()
    }

    #[test]
    fn too_short_reports_parse_error() {
        let mut c = Collect::default();
        assert!(!PhotonParser::new().receive_packet(&[1, 2, 3], &mut c));
        assert!(c.errors[0].0.contains("photon header"));
        assert_eq!(c.errors[0].1, 3);
    }

    #[test]
    fn header_only_zero_commands() {
        assert!(PhotonParser::new().receive_packet(&[0; 12], &mut Collect::default()));
    }

    #[test]
    fn reliable_event() {
        let mut c = Collect::default();
        assert!(PhotonParser::new().receive_packet(&reliable_message_packet(MSG_EVENT, &EVENT_PAYLOAD), &mut c));
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].code, 3);
        assert_eq!(c.events[0].parameters[&252], Value::Byte(3));
    }

    #[test]
    fn fragments() {
        for (n, order) in [(1, vec![0]), (2, vec![0, 1]), (2, vec![1, 0])] {
            let pkts = fragmented_event_packets(n);
            let mut p = PhotonParser::new();
            let mut c = Collect::default();
            for (i, idx) in order.iter().enumerate() {
                assert!(p.receive_packet(&pkts[*idx], &mut c));
                assert_eq!(c.events.len(), usize::from(i + 1 == order.len()));
            }
            assert_eq!(c.events[0].code, 3);
        }
    }

    #[test]
    fn duplicate_fragment_ignored() {
        let pkts = fragmented_event_packets(2);
        let mut p = PhotonParser::new();
        let mut c = Collect::default();
        p.receive_packet(&pkts[0], &mut c);
        p.receive_packet(&pkts[0], &mut c);
        assert!(c.events.is_empty(), "duplicate fragment must not trigger completion");
        p.receive_packet(&pkts[1], &mut c);
        assert_eq!(c.events.len(), 1);
    }

    #[test]
    fn fragment_eviction() {
        let mut p = PhotonParser::new();
        for seq in 0..(MAX_PENDING_SEGMENTS as u32 + 5) {
            let mut frag = Vec::new();
            for v in [seq, 2, 0, 10, 0] {
                frag.extend_from_slice(&v.to_be_bytes());
            }
            frag.extend_from_slice(&[0; 5]);
            p.receive_packet(&single_command_packet(CMD_SEND_FRAGMENT, &frag), &mut Collect::default());
        }
        assert!(p.pending_len() <= MAX_PENDING_SEGMENTS);
    }

    #[test]
    fn unreliable_event() {
        let mut cmd = vec![0, 0, 0, 0, 0x00, MSG_EVENT];
        cmd.extend_from_slice(&EVENT_PAYLOAD);
        let mut c = Collect::default();
        assert!(PhotonParser::new().receive_packet(&single_command_packet(CMD_SEND_UNRELIABLE, &cmd), &mut c));
        assert_eq!(c.events[0].code, 3);
    }

    #[test]
    fn encrypted_message_type_and_flag() {
        let mut c = Collect::default();
        assert!(PhotonParser::new().receive_packet(&reliable_message_packet(MSG_ENCRYPTED, &[0xde, 0xad]), &mut c));
        assert_eq!(c.encrypted, 1);

        let mut c = Collect::default();
        let mut hdr = [0u8; 12];
        hdr[2] = 1;
        assert!(!PhotonParser::new().receive_packet(&hdr, &mut c));
        assert_eq!(c.encrypted, 1);
        assert!(c.errors.is_empty(), "encrypted packet must not count as a parsing error");
    }

    #[test]
    fn bad_command_reports_parse_error() {
        let mut payload = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, CMD_SEND_RELIABLE, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        payload.extend_from_slice(&[0; 4]);
        let mut c = Collect::default();
        assert!(!PhotonParser::new().receive_packet(&payload, &mut c));
        assert!(c.errors[0].0.contains("handleCommand"));
    }

    #[test]
    fn upstream_operation_request_compact_path() {
        let inner = [0x02, 0x02, 253, TYPE_INT_ZERO, 8, TYPE_STRING, 2, b'h', b'i'];
        let mut c = Collect::default();
        assert!(PhotonParser::new().receive_packet(&reliable_message_packet(MSG_REQUEST, &inner), &mut c));
        assert_eq!(c.requests[0].parameters[&253], Value::Int(0));
        assert_eq!(c.requests[0].parameters[&8], Value::String("hi".into()));
    }

    #[test]
    fn upstream_operation_response_return_code() {
        let inner = [42, 0x05, 0x00, TYPE_NULL, 0x00];
        let mut c = Collect::default();
        assert!(PhotonParser::new().receive_packet(&reliable_message_packet(MSG_RESPONSE, &inner), &mut c));
        assert_eq!(c.responses[0].return_code, 5);
    }
}
