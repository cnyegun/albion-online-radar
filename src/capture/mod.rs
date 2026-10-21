//! Live packet capture: interface discovery, per-interface pcap workers and
//! the manager that reconfigures them at runtime.

mod categorize;
pub mod decode;
mod manager;
pub mod pcap_file;

use std::net::{IpAddr, Ipv4Addr};

use anyhow::{Result, anyhow};

pub use categorize::{Category, categorize, rank_candidates};
pub use manager::{Manager, State};

pub const ALBION_PORT: u16 = 5056;
pub const SNAP_LEN: i32 = 65536;
const READ_TIMEOUT_MS: i32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    /// pcap device name (also the identifier persisted in network.json).
    pub name: String,
    pub description: String,
    pub address: String,
}

pub fn enumerate_interfaces() -> Result<Vec<NetworkInterface>> {
    let devices = pcap::Device::list().map_err(|e| anyhow!("list devices: {e}"))?;
    Ok(devices
        .into_iter()
        .filter_map(|d| {
            let v4 = d.addresses.iter().find_map(|a| match a.addr {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            })?;
            Some(NetworkInterface { name: d.name, description: d.desc.unwrap_or_default(), address: v4.to_string() })
        })
        .collect())
}

pub fn is_rfc1918(addr: &str) -> bool {
    match addr.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_private(),
        Ok(IpAddr::V6(ip)) => ip.to_ipv4_mapped().is_some_and(|v4: Ipv4Addr| v4.is_private()),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1918() {
        for (addr, want) in [
            ("192.168.1.2", true),
            ("10.0.0.1", true),
            ("172.16.5.4", true),
            ("172.32.0.1", false),
            ("8.8.8.8", false),
            ("127.0.0.1", false),
            ("::ffff:192.168.0.1", true),
            ("fe80::1", false),
            ("nope", false),
        ] {
            assert_eq!(is_rfc1918(addr), want, "{addr}");
        }
    }
}
