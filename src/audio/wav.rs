//! Minimal RIFF/WAVE reader for the bundled alert sounds.

use anyhow::{Result, anyhow, bail};

pub const CHANNELS: u16 = 2;
pub const SAMPLE_RATE: u32 = 44100;
const BITS: u16 = 16;

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Returns the PCM payload of an uncompressed 16-bit little-endian stereo WAV at 44100 Hz.
pub fn decode_wav(b: &[u8]) -> Result<&[u8]> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let mut pcm = None;
    let mut got_fmt = false;
    let mut off = 12;
    while off + 8 <= b.len() {
        let id = &b[off..off + 4];
        let size = le32(b, off + 4) as usize;
        let body = off + 8;
        let end = body.checked_add(size).filter(|e| *e <= b.len());
        let Some(end) = end else {
            return Err(anyhow!("chunk {:?} runs past end of file", String::from_utf8_lossy(id)));
        };
        match id {
            b"fmt " => {
                if size < 16 {
                    bail!("fmt chunk too short");
                }
                let format = le16(b, body);
                let channels = le16(b, body + 2);
                let rate = le32(b, body + 4);
                let bits = le16(b, body + 14);
                if format != 1 {
                    bail!("want uncompressed PCM, got format {format}");
                }
                if channels != CHANNELS || rate != SAMPLE_RATE || bits != BITS {
                    bail!(
                        "want {CHANNELS} channels {SAMPLE_RATE} Hz {BITS} bit, got {channels} channels {rate} Hz {bits} bit"
                    );
                }
                got_fmt = true;
            }
            b"data" => pcm = Some(&b[body..end]),
            _ => {}
        }
        off = end + (size % 2);
    }
    if !got_fmt {
        bail!("no fmt chunk");
    }
    pcm.ok_or_else(|| anyhow!("no data chunk"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(b"abc\0"); // odd chunk + pad byte
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn decodes_pcm_and_skips_padded_chunks() {
        assert_eq!(decode_wav(&wav(2, 44100, &[1, 2, 3, 4])).unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn rejects_wrong_format() {
        assert!(decode_wav(&wav(1, 44100, &[0; 4])).unwrap_err().to_string().contains("1 channels"));
        assert!(decode_wav(b"nope").is_err());
        let mut truncated = wav(2, 44100, &[0; 4]);
        truncated.truncate(truncated.len() - 2);
        assert!(decode_wav(&truncated).unwrap_err().to_string().contains("past end"));
    }

    #[test]
    fn bundled_sounds_decode() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/sounds");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let raw = std::fs::read(&path).unwrap();
            decode_wav(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}
