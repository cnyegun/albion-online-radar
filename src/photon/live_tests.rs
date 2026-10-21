//! Replays anonymized live Albion captures (testdata/photon) through the parser.
//! Real IPs, MACs and timestamps were rewritten; UDP payloads are intact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::capture::decode::udp_payload;
use crate::capture::pcap_file::read_pcap;

#[derive(Default)]
struct Op {
    real_code: i64,
    keys: Vec<u8>,
}

#[derive(Default)]
struct Stats {
    events: HashMap<u8, usize>,
    requests: HashMap<u8, usize>,
    responses: HashMap<u8, usize>,
    ops: HashMap<&'static str, Vec<Op>>,
}

impl Stats {
    fn record(&mut self, kind: &'static str, code_key: u8, params: &Params) {
        let real_code = params.get(&code_key).and_then(Value::as_i64).unwrap_or(-1);
        self.ops.entry(kind).or_default().push(Op { real_code, keys: params.keys().copied().collect() });
    }

    fn count(&self, kind: &str, code: i64) -> usize {
        self.ops.get(kind).map_or(0, |ops| ops.iter().filter(|o| o.real_code == code).count())
    }

    fn has(&self, kind: &str, code: i64, keys: &[u8]) -> bool {
        self.ops
            .get(kind)
            .is_some_and(|ops| ops.iter().any(|o| o.real_code == code && keys.iter().all(|k| o.keys.contains(k))))
    }
}

impl PhotonHandler for Stats {
    fn on_event(&mut self, mut e: EventData) {
        post_process_event(&mut e);
        *self.events.entry(e.code).or_default() += 1;
        self.record("event", 252, &e.parameters);
    }
    fn on_request(&mut self, mut r: OperationRequest) {
        post_process_request(&mut r);
        *self.requests.entry(r.operation_code).or_default() += 1;
        self.record("request", 253, &r.parameters);
    }
    fn on_response(&mut self, mut r: OperationResponse) {
        post_process_response(&mut r);
        *self.responses.entry(r.operation_code).or_default() += 1;
        self.record("response", 253, &r.parameters);
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/photon")
}

fn replay_into(path: &Path, stats: &mut Stats) {
    let file = read_pcap(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut parser = PhotonParser::new();
    for frame in &file.packets {
        if let Some(payload) = udp_payload(file.link_type, frame) {
            parser.receive_packet(payload, stats);
        }
    }
}

fn replay(name: &str) -> Stats {
    let mut stats = Stats::default();
    replay_into(&fixtures_dir().join(name), &mut stats);
    stats
}

/// Every fixture, recursively, in sorted order.
fn all_fixtures() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "pcap") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&fixtures_dir(), &mut out);
    out.sort();
    out
}

#[test]
fn move_heavy() {
    let s = replay("move_heavy.pcap");
    assert!(s.events.get(&3).copied().unwrap_or(0) >= 100, "{:?}", s.events);
    assert!(s.has("event", 3, &[4, 5]), "a Move event must carry finite params 4 and 5");
}

#[test]
fn generic_events() {
    let s = replay("generic_events.pcap");
    assert!(s.events.get(&1).copied().unwrap_or(0) >= 50, "{:?}", s.events);
    assert!(s.has("event", 29, &[1]), "NewCharacter with a name");
}

#[test]
fn operations() {
    let s = replay("operations.pcap");
    assert!(s.count("request", 22) >= 100);
    assert!(s.has("request", 22, &[0, 1, 3, 253]));
}

#[test]
fn fragments() {
    let s = replay("fragments.pcap");
    let decoded: usize = s.events.values().chain(s.requests.values()).chain(s.responses.values()).sum();
    assert!(decoded > 0);
}

#[test]
fn router_contract() {
    let s = replay("move_map_change.pcap");
    for (kind, code, min, keys) in [
        ("request", 22, 50, &[0u8, 1, 3, 253][..]),
        ("response", 2, 1, &[8, 9, 253][..]),
        ("response", 41, 1, &[0, 253][..]),
    ] {
        assert!(s.count(kind, code) >= min, "{kind} {code}");
        assert!(s.has(kind, code, keys), "{kind} {code} {keys:?}");
    }
}

#[test]
fn all_fixtures_decode() {
    let mut stats = Stats::default();
    let fixtures = all_fixtures();
    assert!(fixtures.len() >= 20);
    for path in &fixtures {
        replay_into(path, &mut stats);
    }
    let decoded: usize = stats.events.values().chain(stats.requests.values()).chain(stats.responses.values()).sum();
    assert!(decoded > 3000, "{decoded}");
}
