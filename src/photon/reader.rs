//! Bounds-checked cursor over a Photon payload.
//!
//! Every read is total: running off the end yields zero values instead of a
//! panic, matching the forgiving behaviour the protocol decoder relies on.

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes left to read.
    pub fn len(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// Takes up to `n` bytes (fewer when the buffer runs out).
    pub fn next(&mut self, n: usize) -> &'a [u8] {
        let end = self.pos + n.min(self.len());
        let out = &self.buf[self.pos..end];
        self.pos = end;
        out
    }

    /// Fills `dst` with as many bytes as remain; the tail stays zeroed.
    pub fn read_into(&mut self, dst: &mut [u8]) {
        let src = self.next(dst.len());
        dst[..src.len()].copy_from_slice(src);
    }

    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.next(N).try_into().ok()
    }

    pub fn compressed_u32(&mut self) -> u32 {
        let mut value: u32 = 0;
        let mut shift = 0u32;
        loop {
            let Some(b) = self.read_byte() else { return 0 };
            value |= u32::from(b & 0x7f).wrapping_shl(shift);
            if b & 0x80 == 0 {
                return value;
            }
            shift += 7;
            if shift >= 35 {
                return 0;
            }
        }
    }

    pub fn compressed_u64(&mut self) -> u64 {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let Some(b) = self.read_byte() else { return 0 };
            value |= u64::from(b & 0x7f).wrapping_shl(shift);
            if b & 0x80 == 0 {
                return value;
            }
            shift += 7;
            if shift >= 70 {
                return 0;
            }
        }
    }

    pub fn compressed_i32(&mut self) -> i32 {
        let v = self.compressed_u32();
        ((v >> 1) ^ (v & 1).wrapping_neg()) as i32
    }

    pub fn compressed_i64(&mut self) -> i64 {
        let v = self.compressed_u64();
        ((v >> 1) ^ (v & 1).wrapping_neg()) as i64
    }

    pub fn count(&mut self) -> usize {
        self.compressed_u32() as usize
    }

    pub fn i16(&mut self) -> i16 {
        self.fixed().map_or(0, i16::from_le_bytes)
    }

    pub fn u16(&mut self) -> u16 {
        self.fixed().map_or(0, u16::from_le_bytes)
    }

    pub fn f32(&mut self) -> f32 {
        self.fixed().map_or(0.0, f32::from_le_bytes)
    }

    pub fn f64(&mut self) -> f64 {
        self.fixed().map_or(0.0, f64::from_le_bytes)
    }

    /// Length-prefixed string. Invalid UTF-8 is replaced, not rejected.
    pub fn string(&mut self) -> String {
        let len = self.compressed_u32() as usize;
        if len == 0 || len > self.len() {
            return String::new();
        }
        String::from_utf8_lossy(self.next(len)).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_u32() {
        let cases: &[(&[u8], u32)] = &[
            (&[0x00], 0),
            (&[0x7f], 127),
            (&[0x80, 0x01], 128),
            (&[0xff, 0x7f], 16383),
            (&[0x80, 0x80, 0x01], 16384),
            (&[0xff, 0xff, 0xff, 0xff, 0x0f], u32::MAX),
            (&[0x80], 0),
            (&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff], 0),
        ];
        for (input, want) in cases {
            assert_eq!(Reader::new(input).compressed_u32(), *want, "{input:?}");
        }
    }

    #[test]
    fn compressed_i32_zigzag() {
        let cases: &[(&[u8], i32)] = &[
            (&[0x00], 0),
            (&[0x02], 1),
            (&[0x01], -1),
            (&[0x04], 2),
            (&[0x03], -2),
            (&[0xfe, 0xff, 0xff, 0xff, 0x0f], i32::MAX),
            (&[0xff, 0xff, 0xff, 0xff, 0x0f], i32::MIN),
        ];
        for (input, want) in cases {
            assert_eq!(Reader::new(input).compressed_i32(), *want, "{input:?}");
        }
    }

    #[test]
    fn little_endian_primitives() {
        assert_eq!(Reader::new(&[0x34, 0x12]).i16(), 0x1234);
        assert_eq!(Reader::new(&[0x00, 0x00, 0x80, 0x3f]).f32(), 1.0);
        assert_eq!(Reader::new(&[0, 0, 0, 0, 0, 0, 0xf0, 0x3f]).f64(), 1.0);
        assert_eq!(Reader::new(&[0x34]).i16(), 0);
    }

    #[test]
    fn strings() {
        assert_eq!(Reader::new(&[0x05, b'h', b'e', b'l', b'l', b'o']).string(), "hello");
        assert_eq!(Reader::new(&[0x00]).string(), "");
        assert_eq!(Reader::new(&[0x0a, b'h', b'i', b'!']).string(), "");
        assert_eq!(Reader::new(&[0x03, b'a', 0xff, b'b']).string(), "a\u{fffd}b");
    }
}
