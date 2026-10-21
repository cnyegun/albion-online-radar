//! Classic libpcap capture files (used for replay and tests).

use std::io;
use std::path::Path;

const MAGIC_MICROS: u32 = 0xa1b2_c3d4;
const MAGIC_NANOS: u32 = 0xa1b2_3c4d;

/// Every record of a pcap file, in memory.
pub struct PcapFile {
    pub link_type: i32,
    pub packets: Vec<Vec<u8>>,
    /// Capture time of each packet, in seconds.
    pub times: Vec<f64>,
}

pub fn read_pcap(path: &Path) -> io::Result<PcapFile> {
    let raw = std::fs::read(path)?;
    parse_pcap(&raw).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a classic pcap file"))
}

fn parse_pcap(raw: &[u8]) -> Option<PcapFile> {
    let magic: [u8; 4] = raw.get(0..4)?.try_into().ok()?;
    let (le, nanos) = match (u32::from_le_bytes(magic), u32::from_be_bytes(magic)) {
        (MAGIC_MICROS, _) => (true, false),
        (MAGIC_NANOS, _) => (true, true),
        (_, MAGIC_MICROS) => (false, false),
        (_, MAGIC_NANOS) => (false, true),
        _ => return None,
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = raw.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let frac = if nanos { 1e-9 } else { 1e-6 };
    let link_type = (u32_at(20)? & 0x0fff_ffff) as i32;
    let (mut packets, mut times) = (Vec::new(), Vec::new());
    let mut at = 24;
    while at + 16 <= raw.len() {
        let len = u32_at(at + 8)? as usize;
        let Some(data) = raw.get(at + 16..at + 16 + len) else { break };
        times.push(f64::from(u32_at(at)?) + f64::from(u32_at(at + 4)?) * frac);
        packets.push(data.to_vec());
        at += 16 + len;
    }
    Some(PcapFile { link_type, packets, times })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ts: u32, data: &[u8]) -> Vec<u8> {
        let mut r = Vec::new();
        for v in [ts, 500_000, data.len() as u32, data.len() as u32] {
            r.extend_from_slice(&v.to_le_bytes());
        }
        r.extend_from_slice(data);
        r
    }

    #[test]
    fn parses_records_and_times() {
        let mut raw = Vec::new();
        for v in [MAGIC_MICROS, 0x0004_0002, 0, 0, 65536, 1] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        raw.extend(record(10, &[1, 2, 3]));
        raw.extend(record(11, &[9; 4]));
        raw.extend_from_slice(&[0; 7]); // truncated trailing record is ignored
        let f = parse_pcap(&raw).unwrap();
        assert_eq!(f.link_type, 1);
        assert_eq!(f.packets, vec![vec![1, 2, 3], vec![9; 4]]);
        assert_eq!(f.times, vec![10.5, 11.5]);
        assert!(parse_pcap(b"nope").is_none());
    }
}
