//! Minimal link-layer -> IP -> UDP decoding, enough to pull the Photon payload
//! out of a captured frame. Anything unexpected yields `None`.

pub const LINKTYPE_NULL: i32 = 0;
pub const LINKTYPE_ETHERNET: i32 = 1;
const LINKTYPE_RAW_BSD: i32 = 12;
const LINKTYPE_RAW_BSD_ALT: i32 = 14;
const LINKTYPE_RAW: i32 = 101;
const LINKTYPE_LOOP: i32 = 108;
const LINKTYPE_LINUX_SLL: i32 = 113;
const LINKTYPE_IPV4: i32 = 228;
const LINKTYPE_IPV6: i32 = 229;
const LINKTYPE_LINUX_SLL2: i32 = 276;

const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_IPV6: u16 = 0x86dd;
const ETHERTYPE_VLAN: u16 = 0x8100;
const ETHERTYPE_QINQ: u16 = 0x88a8;

const IPPROTO_UDP: u8 = 17;

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// Returns the UDP payload carried by a captured frame.
pub fn udp_payload(link_type: i32, frame: &[u8]) -> Option<&[u8]> {
    match link_type {
        LINKTYPE_ETHERNET => {
            let mut ethertype = be16(frame, 12)?;
            let mut at = 14;
            while ethertype == ETHERTYPE_VLAN || ethertype == ETHERTYPE_QINQ {
                ethertype = be16(frame, at + 2)?;
                at += 4;
            }
            by_ethertype(ethertype, frame.get(at..)?)
        }
        LINKTYPE_NULL | LINKTYPE_LOOP => {
            // 4-byte address family in host (NULL) or network (LOOP) order;
            // the IP version nibble tells us what follows either way.
            ip(frame.get(4..)?)
        }
        LINKTYPE_RAW | LINKTYPE_RAW_BSD | LINKTYPE_RAW_BSD_ALT | LINKTYPE_IPV4 | LINKTYPE_IPV6 => ip(frame),
        LINKTYPE_LINUX_SLL => by_ethertype(be16(frame, 14)?, frame.get(16..)?),
        LINKTYPE_LINUX_SLL2 => by_ethertype(be16(frame, 0)?, frame.get(20..)?),
        _ => None,
    }
}

fn by_ethertype(ethertype: u16, data: &[u8]) -> Option<&[u8]> {
    match ethertype {
        ETHERTYPE_IPV4 => ipv4(data),
        ETHERTYPE_IPV6 => ipv6(data),
        _ => None,
    }
}

fn ip(data: &[u8]) -> Option<&[u8]> {
    match data.first()? >> 4 {
        4 => ipv4(data),
        6 => ipv6(data),
        _ => None,
    }
}

fn ipv4(data: &[u8]) -> Option<&[u8]> {
    let ihl = usize::from(data.first()? & 0x0f) * 4;
    let total_len = usize::from(be16(data, 2)?);
    if ihl < 20 || total_len < ihl || data.len() < ihl {
        return None;
    }
    // Fragmented datagrams carry no complete UDP header past the first piece,
    // and the first piece alone is not a whole Photon packet either.
    let frag = be16(data, 6)?;
    if frag & 0x3fff != 0 {
        return None;
    }
    if *data.get(9)? != IPPROTO_UDP {
        return None;
    }
    // Trim Ethernet padding using the IP total length.
    let end = total_len.min(data.len());
    udp(&data[ihl..end])
}

fn ipv6(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 40 {
        return None;
    }
    let payload_len = usize::from(be16(data, 4)?);
    let mut next = data[6];
    let end = (40 + payload_len).min(data.len());
    let mut body = &data[40..end];
    loop {
        match next {
            IPPROTO_UDP => return udp(body),
            // Hop-by-hop, routing, destination options.
            0 | 43 | 60 => {
                next = *body.first()?;
                let len = (usize::from(*body.get(1)?) + 1) * 8;
                body = body.get(len..)?;
            }
            _ => return None,
        }
    }
}

fn udp(data: &[u8]) -> Option<&[u8]> {
    let length = usize::from(be16(data, 4)?);
    match length {
        0 => data.get(8..),
        8.. => data.get(8..length.min(data.len())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eth_ipv4_udp(payload: &[u8], padding: usize) -> Vec<u8> {
        let mut f = vec![0u8; 12];
        f.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        let total = 20 + 8 + payload.len();
        let mut ip = vec![0x45, 0, 0, 0, 0, 0, 0x40, 0, 64, IPPROTO_UDP, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
        ip[2..4].copy_from_slice(&(total as u16).to_be_bytes());
        f.extend_from_slice(&ip);
        let mut udp = vec![0x13, 0xc0, 0x13, 0xc0, 0, 0, 0, 0];
        udp[4..6].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
        f.extend_from_slice(&udp);
        f.extend_from_slice(payload);
        f.extend(std::iter::repeat_n(0xee, padding));
        f
    }

    #[test]
    fn ethernet_ipv4_udp_trims_padding() {
        let frame = eth_ipv4_udp(b"hi", 10);
        assert_eq!(udp_payload(LINKTYPE_ETHERNET, &frame), Some(&b"hi"[..]));
    }

    #[test]
    fn vlan_tag_is_skipped() {
        let plain = eth_ipv4_udp(b"abc", 0);
        let mut tagged = plain[..12].to_vec();
        tagged.extend_from_slice(&[0x81, 0x00, 0x00, 0x05]);
        tagged.extend_from_slice(&plain[12..]);
        assert_eq!(udp_payload(LINKTYPE_ETHERNET, &tagged), Some(&b"abc"[..]));
    }

    #[test]
    fn raw_ip_and_fragments() {
        let frame = eth_ipv4_udp(b"xyz", 0);
        assert_eq!(udp_payload(LINKTYPE_RAW, &frame[14..]), Some(&b"xyz"[..]));
        let mut frag = frame.clone();
        frag[14 + 6] = 0x20; // more fragments
        assert_eq!(udp_payload(LINKTYPE_ETHERNET, &frag), None);
    }

    #[test]
    fn truncated_frames_do_not_panic() {
        let frame = eth_ipv4_udp(b"payload", 0);
        for n in 0..frame.len() {
            let _ = udp_payload(LINKTYPE_ETHERNET, &frame[..n]);
        }
    }
}
