//! Owns one capture worker thread per selected interface.
//!
//! Each pcap handle lives entirely inside its worker thread and is dropped by
//! that thread when it exits, so a handle can never be closed while a read on
//! it is still in flight.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

use super::decode::udp_payload;
use super::{ALBION_PORT, NetworkInterface, READ_TIMEOUT_MS, SNAP_LEN};

/// Called with each UDP payload seen on any active interface.
pub type PacketHandler = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct State {
    /// Names of the interfaces being captured, sorted.
    pub active: Vec<String>,
    /// Last open/read error per interface name.
    pub last_errors: HashMap<String, String>,
}

enum ReadOutcome<'a> {
    Frame(&'a [u8]),
    Timeout,
}

/// A packet source owned by a worker thread. Live captures use pcap; tests
/// substitute a stub.
trait PacketSource: Send {
    fn link_type(&self) -> i32;
    fn next(&mut self) -> Result<ReadOutcome<'_>>;
}

struct LiveSource(pcap::Capture<pcap::Active>);

impl PacketSource for LiveSource {
    fn link_type(&self) -> i32 {
        self.0.get_datalink().0
    }

    fn next(&mut self) -> Result<ReadOutcome<'_>> {
        match self.0.next_packet() {
            Ok(p) => Ok(ReadOutcome::Frame(p.data)),
            Err(pcap::Error::TimeoutExpired) => Ok(ReadOutcome::Timeout),
            Err(e) => Err(anyhow!(e)),
        }
    }
}

type Opener = Arc<dyn Fn(&NetworkInterface) -> Result<Box<dyn PacketSource>> + Send + Sync>;

fn open_live(iface: &NetworkInterface) -> Result<Box<dyn PacketSource>> {
    let mut cap = pcap::Capture::from_device(iface.name.as_str())
        .and_then(|c| c.snaplen(SNAP_LEN).promisc(false).timeout(READ_TIMEOUT_MS).open())
        .map_err(|e| anyhow!("open device {:?}: {e}", iface.name))?;
    let filter = format!("udp and (dst port {ALBION_PORT} or src port {ALBION_PORT})");
    cap.filter(&filter, true).map_err(|e| anyhow!("set BPF filter on {:?}: {e}", iface.name))?;
    Ok(Box::new(LiveSource(cap)))
}

struct Worker {
    id: u64,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    active: HashMap<String, Worker>,
    on_packet: Option<PacketHandler>,
    last_errors: HashMap<String, String>,
    closed: bool,
    next_id: u64,
}

pub struct Manager {
    inner: Arc<Mutex<Inner>>,
    opener: Opener,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub fn new() -> Self {
        Self::with_opener(Arc::new(open_live))
    }

    fn with_opener(opener: Opener) -> Self {
        Self { inner: Arc::default(), opener }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn on_packet(&self, h: PacketHandler) {
        self.lock().on_packet = Some(h);
    }

    /// Opens captures for interfaces in `target` that are not running yet and
    /// stops the ones no longer wanted.
    pub fn reconfigure(&self, target: &[NetworkInterface]) -> Result<()> {
        let mut inner = self.lock();
        if inner.closed {
            bail!("manager closed");
        }
        let Some(on_packet) = inner.on_packet.clone() else {
            bail!("on_packet must be set before reconfigure");
        };

        let mut open_errs = Vec::new();
        for iface in target {
            if inner.active.contains_key(&iface.name) {
                continue;
            }
            let source = match (self.opener)(iface) {
                Ok(s) => s,
                Err(e) => {
                    inner.last_errors.insert(iface.name.clone(), e.to_string());
                    open_errs.push(format!("{}: {e}", iface.name));
                    continue;
                }
            };
            inner.last_errors.remove(&iface.name);
            inner.next_id += 1;
            let id = inner.next_id;
            let stop = Arc::new(AtomicBool::new(false));
            let thread =
                spawn_worker(id, iface.name.clone(), source, Arc::clone(&stop), on_packet.clone(), Arc::clone(&self.inner));
            inner.active.insert(iface.name.clone(), Worker { id, stop, thread });
        }

        // Dropped workers exit within one read timeout and close their handle.
        let wanted: Vec<&str> = target.iter().map(|i| i.name.as_str()).collect();
        inner.active.retain(|name, w| {
            let keep = wanted.contains(&name.as_str());
            if !keep {
                w.stop.store(true, Ordering::Release);
            }
            keep
        });
        inner.last_errors.retain(|name, _| wanted.contains(&name.as_str()));

        if open_errs.is_empty() { Ok(()) } else { Err(anyhow!("could not open: {}", open_errs.join("; "))) }
    }

    pub fn state(&self) -> State {
        let inner = self.lock();
        let mut active: Vec<String> = inner.active.keys().cloned().collect();
        active.sort();
        State { active, last_errors: inner.last_errors.clone() }
    }

    /// Stops every worker and waits up to `timeout` for them to exit.
    pub fn close(&self, timeout: Duration) {
        let workers: Vec<Worker> = {
            let mut inner = self.lock();
            if inner.closed {
                return;
            }
            inner.closed = true;
            inner.active.drain().map(|(_, w)| w).collect()
        };
        for w in &workers {
            w.stop.store(true, Ordering::Release);
        }
        let deadline = Instant::now() + timeout;
        for w in workers {
            while !w.thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if w.thread.is_finished() {
                let _ = w.thread.join();
            }
        }
    }
}

fn spawn_worker(
    id: u64,
    name: String,
    mut source: Box<dyn PacketSource>,
    stop: Arc<AtomicBool>,
    on_packet: PacketHandler,
    inner: Arc<Mutex<Inner>>,
) -> JoinHandle<()> {
    let link_type = source.link_type();
    std::thread::Builder::new()
        .name(format!("capture-{name}"))
        .spawn(move || {
            let mut result = Ok(());
            while !stop.load(Ordering::Acquire) {
                match source.next() {
                    Ok(ReadOutcome::Frame(frame)) => {
                        if let Some(payload) = udp_payload(link_type, frame).filter(|p| !p.is_empty()) {
                            on_packet(payload);
                        }
                    }
                    Ok(ReadOutcome::Timeout) => {}
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            if let Err(e) = result {
                let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
                // Only drop our own entry; the interface may have been re-added since.
                if inner.active.get(&name).is_some_and(|w| w.id == id) {
                    inner.active.remove(&name);
                    inner.last_errors.insert(name, e.to_string());
                }
            }
        })
        .expect("spawn capture thread")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::super::decode::LINKTYPE_ETHERNET;
    use super::*;

    struct StubSource {
        frame: Option<Vec<u8>>,
        fail_after: Option<usize>,
        reads: usize,
    }

    impl PacketSource for StubSource {
        fn link_type(&self) -> i32 {
            LINKTYPE_ETHERNET
        }
        fn next(&mut self) -> Result<ReadOutcome<'_>> {
            self.reads += 1;
            if self.fail_after.is_some_and(|n| self.reads > n) {
                bail!("device went away");
            }
            match &self.frame {
                Some(f) => Ok(ReadOutcome::Frame(f)),
                None => {
                    std::thread::sleep(Duration::from_millis(5));
                    Ok(ReadOutcome::Timeout)
                }
            }
        }
    }

    fn iface(name: &str) -> NetworkInterface {
        NetworkInterface { name: name.into(), description: format!("{name} adapter"), address: "192.168.1.2".into() }
    }

    fn stub_manager(frame: Option<Vec<u8>>, fail_after: Option<usize>, fail_open: &'static [&'static str]) -> Manager {
        let m = Manager::with_opener(Arc::new(move |i: &NetworkInterface| {
            if fail_open.contains(&i.name.as_str()) {
                bail!("no such device");
            }
            Ok(Box::new(StubSource { frame: frame.clone(), fail_after, reads: 0 }) as Box<dyn PacketSource>)
        }));
        m.on_packet(Arc::new(|_| {}));
        m
    }

    fn wait_until_idle(m: &Manager) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !m.state().active.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reconfigure_adds_and_removes() {
        let m = stub_manager(None, None, &[]);
        m.reconfigure(&[iface("wlan0"), iface("eth0")]).unwrap();
        assert_eq!(m.state().active, ["eth0", "wlan0"]);
        m.reconfigure(&[iface("wlan0")]).unwrap();
        assert_eq!(m.state().active, ["wlan0"]);
        m.reconfigure(&[]).unwrap();
        assert!(m.state().active.is_empty());
        m.close(Duration::from_secs(1));
        assert!(m.reconfigure(&[iface("eth0")]).is_err());
    }

    #[test]
    fn open_failures_are_reported_per_interface() {
        let m = stub_manager(None, None, &["bad0"]);
        let err = m.reconfigure(&[iface("eth0"), iface("bad0")]).unwrap_err();
        assert!(err.to_string().contains("bad0"));
        let s = m.state();
        assert_eq!(s.active, ["eth0"]);
        assert!(s.last_errors.contains_key("bad0"));
        m.close(Duration::from_secs(1));
    }

    #[test]
    fn worker_error_removes_capture() {
        let m = stub_manager(None, Some(3), &[]);
        m.reconfigure(&[iface("eth0")]).unwrap();
        wait_until_idle(&m);
        let s = m.state();
        assert!(s.active.is_empty());
        assert!(s.last_errors["eth0"].contains("went away"));
        m.close(Duration::from_secs(1));
    }

    #[test]
    fn requires_packet_handler() {
        let m = Manager::with_opener(Arc::new(|_: &NetworkInterface| bail!("unused")));
        assert!(m.reconfigure(&[iface("eth0")]).is_err());
    }

    #[test]
    fn udp_payloads_reach_the_handler() {
        // Ethernet/IPv4/UDP frame carrying "hey".
        let mut frame = vec![0u8; 12];
        frame.extend_from_slice(&[0x08, 0x00, 0x45, 0, 0, 31, 0, 0, 0, 0, 64, 17, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2]);
        frame.extend_from_slice(&[0x13, 0xc0, 0x13, 0xc0, 0, 11, 0, 0, b'h', b'e', b'y']);

        let m = stub_manager(Some(frame), Some(50), &[]);
        let seen = Arc::new(AtomicUsize::new(0));
        let seen2 = Arc::clone(&seen);
        m.on_packet(Arc::new(move |p| {
            assert_eq!(p, b"hey");
            seen2.fetch_add(1, Ordering::Relaxed);
        }));
        m.reconfigure(&[iface("eth0")]).unwrap();
        wait_until_idle(&m);
        assert_eq!(seen.load(Ordering::Relaxed), 50);
        m.close(Duration::from_secs(1));
    }
}
