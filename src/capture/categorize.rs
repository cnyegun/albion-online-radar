//! Classifies capture interfaces so sensible defaults can be picked.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::NetworkInterface;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Wifi,
    Ethernet,
    Exitlag,
    Vpn,
    Virtual,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Wifi => "wifi",
            Category::Ethernet => "ethernet",
            Category::Exitlag => "exitlag",
            Category::Vpn => "vpn",
            Category::Virtual => "virtual",
            Category::Other => "other",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Category::Ethernet => 0,
            Category::Wifi => 1,
            Category::Exitlag => 2,
            Category::Vpn => 3,
            Category::Virtual => 4,
            Category::Other => 5,
        }
    }
}

// Order matters: Virtual first overrides Wi-Fi/Ethernet substrings; ExitLag before VPN.
// `(?-u:\b)` keeps word boundaries ASCII-only, like Go's RE2.
static RULES: LazyLock<Vec<(Category, Regex)>> = LazyLock::new(|| {
    let b = r"(?-u:\b)";
    [
        (
            Category::Virtual,
            format!(
                r"virtualbox|vmware|hyper-v|virtual switch|vethernet|teredo|loopback pseudo|software loopback|wi-fi direct|mobile hotspot|{b}docker\d|{b}br-|{b}virbr\d|{b}vmnet\d|{b}veth|{b}lo{b}"
            ),
        ),
        (Category::Exitlag, r"exit\s*lag".to_owned()),
        (Category::Vpn, format!(r"vpn|wireguard|wintun|tap-windows|openvpn|{b}tun\d|{b}tap\d|{b}wg\d|{b}ppp\d")),
        (Category::Wifi, format!(r"wi-?fi|wireless|802\.11|{b}wlan\d|{b}wlp\d|{b}wifi\d")),
        (Category::Ethernet, format!(r"ethernet|gigabit|family controller|{b}eth\d|{b}enp\d|{b}eno\d|{b}ens\d")),
    ]
    .into_iter()
    .map(|(cat, re)| (cat, Regex::new(&re).expect("static regex")))
    .collect()
});

pub fn categorize(name: &str, description: &str) -> Category {
    if name.is_empty() && description.is_empty() {
        return Category::Other;
    }
    let hay = format!("{name} {description}").to_lowercase();
    RULES.iter().find(|(_, re)| re.is_match(&hay)).map_or(Category::Other, |(cat, _)| *cat)
}

pub fn category_rank(name: &str, description: &str) -> u8 {
    categorize(name, description).rank()
}

/// Best candidates first: by category, then interfaces with a description.
pub fn rank_candidates(input: &[NetworkInterface]) -> Vec<NetworkInterface> {
    let mut out = input.to_vec();
    out.sort_by_key(|i| (category_rank(&i.name, &i.description), i.description.is_empty()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        let cases = [
            ("eth0", "", Category::Ethernet),
            ("enp3s0", "", Category::Ethernet),
            ("wlan0", "", Category::Wifi),
            ("wlp2s0", "", Category::Wifi),
            (r"\Device\NPF_{X}", "Intel(R) Wi-Fi 6 AX201 160MHz", Category::Wifi),
            (r"\Device\NPF_{X}", "Realtek PCIe GbE Family Controller", Category::Ethernet),
            (r"\Device\NPF_{X}", "Hyper-V Virtual Ethernet Adapter", Category::Virtual),
            (r"\Device\NPF_{X}", "ExitLag Tunnel", Category::Exitlag),
            ("docker0", "", Category::Virtual),
            ("lo", "", Category::Virtual),
            ("tun0", "", Category::Vpn),
            ("wg0", "", Category::Vpn),
            ("bluetooth0", "", Category::Other),
            ("", "", Category::Other),
        ];
        for (name, desc, want) in cases {
            assert_eq!(categorize(name, desc), want, "{name} / {desc}");
        }
    }

    #[test]
    fn ranking_is_stable_and_prefers_descriptions() {
        let mk = |n: &str, d: &str| NetworkInterface { name: n.into(), description: d.into(), address: String::new() };
        let ranked = rank_candidates(&[mk("docker0", ""), mk("wlan0", ""), mk("eth1", ""), mk("eth0", "Onboard")]);
        let names: Vec<_> = ranked.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["eth0", "eth1", "wlan0", "docker0"]);
    }
}
