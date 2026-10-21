//! Classifies capture interfaces so sensible defaults can be picked.

use super::NetworkInterface;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Ethernet,
    Wifi,
    Exitlag,
    Vpn,
    Virtual,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Ethernet => "ethernet",
            Category::Wifi => "wifi",
            Category::Exitlag => "exitlag",
            Category::Vpn => "vpn",
            Category::Virtual => "virtual",
            Category::Other => "other",
        }
    }
}

/// Linux interface name prefixes followed by a digit (`wlp2s0`, `docker0`...).
const NAME_RULES: [(Category, &[&str]); 4] = [
    (Category::Virtual, &["docker", "virbr", "vmnet"]),
    (Category::Vpn, &["tun", "tap", "wg", "ppp"]),
    (Category::Wifi, &["wlan", "wlp", "wifi"]),
    (Category::Ethernet, &["eth", "enp", "eno", "ens"]),
];

/// Lowercase substrings of the name or description. Checked first, in order.
const TEXT_RULES: [(Category, &[&str]); 5] = [
    (Category::Virtual, &["virtual", "vmware", "hyper-v", "loopback"]),
    (Category::Exitlag, &["exitlag", "exit lag"]),
    (Category::Vpn, &["vpn", "wireguard"]),
    (Category::Wifi, &["wi-fi", "wifi", "wireless", "802.11"]),
    (Category::Ethernet, &["ethernet", "gigabit"]),
];

pub fn categorize(name: &str, description: &str) -> Category {
    let name = name.to_lowercase();
    if name == "lo" || name.starts_with("br-") || name.starts_with("veth") {
        return Category::Virtual;
    }
    let text = format!("{name} {}", description.to_lowercase());
    if let Some((cat, _)) = TEXT_RULES.iter().find(|(_, words)| words.iter().any(|w| text.contains(w))) {
        return *cat;
    }
    let numbered = |p: &str| name.strip_prefix(p).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    NAME_RULES
        .iter()
        .find(|(_, prefixes)| prefixes.iter().any(|p| numbered(p)))
        .map_or(Category::Other, |(cat, _)| *cat)
}

/// Best candidates first: by category, then interfaces with a description.
pub fn rank_candidates(input: &[NetworkInterface]) -> Vec<NetworkInterface> {
    let mut out = input.to_vec();
    out.sort_by_key(|i| (categorize(&i.name, &i.description), i.description.is_empty()));
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
            ("wlp1s0", "Intel Wi-Fi 6 AX201", Category::Wifi),
            ("eno1", "Realtek Gigabit Ethernet", Category::Ethernet),
            ("vboxnet0", "VirtualBox Host-Only Ethernet", Category::Virtual),
            ("exitlag0", "", Category::Exitlag),
            ("docker0", "", Category::Virtual),
            ("br-1a2b3c", "", Category::Virtual),
            ("veth12ab", "", Category::Virtual),
            ("lo", "", Category::Virtual),
            ("tun0", "", Category::Vpn),
            ("wg0", "", Category::Vpn),
            ("tailscale0", "", Category::Other),
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
