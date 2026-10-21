//! A lossless WebP of many groups of Huffman codes, for the tests of the limit on them (in
//! `tests.rs` beside this, and `tests/image_memory.rs`).

/// Bits written the way a lossless WebP is read: each value's lowest bit first.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    n: usize,
}

impl Bits {
    fn put(&mut self, value: u32, bits: usize) {
        for i in 0..bits {
            if self.n.is_multiple_of(8) {
                self.out.push(0);
            }
            *self.out.last_mut().unwrap() |= ((value >> i & 1) as u8) << (self.n % 8);
            self.n += 1;
        }
    }

    /// A Huffman code `code` of `len` bits, which is read from its highest bit.
    fn code(&mut self, code: u32, len: usize) {
        for i in (0..len).rev() {
            self.put(code >> i & 1, 1);
        }
    }

    /// A code of one symbol, which takes no bits to read.
    fn simple(&mut self, symbol: u8) {
        self.put(0b101, 3);
        self.put(symbol.into(), 8);
    }

    /// A code of 11 symbols of 1 to 10 bits: a table of 1,024 entries.
    fn full(&mut self) {
        // Not simple; 14 code length code lengths, in the format's order (17, 18, 0, 1, 2, 3,
        // 4, 5, 16, 6, 7, 8, 9, 10): lengths 1–6 take 3 bits, 7–10 take 4.
        self.put(0, 1);
        self.put(10, 4);
        for len in [0, 0, 0, 3, 3, 3, 3, 3, 0, 3, 4, 4, 4, 4] {
            self.put(len, 3);
        }
        // 11 symbols (4 bits of 11 − 2), of lengths 1, 2, ..., 10, 10.
        self.put(1, 1);
        self.put(1, 3);
        self.put(9, 4);
        for len in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10] {
            // Canonical: 1–6 are 0–5 in 3 bits, 7–10 are 12–15 in 4.
            if len <= 6 {
                self.code(len - 1, 3);
            } else {
                self.code(len + 5, 4);
            }
        }
    }
}

/// A lossless WebP of `side` × `side` whose pixels all use the last of `groups` groups of
/// Huffman codes: the others, read and never used, are five codes of 1,024-entry tables each
/// (20 KB a group, from 58 bytes of the file).
pub fn many_groups(groups: u32, side: u32) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(0x2F, 8);
    b.put(side - 1, 14);
    b.put(side - 1, 14);
    b.put(0, 4);
    // No transform, no color cache; groups by blocks of 4 × 4, whose image of one color says
    // group `groups − 1` (in its red and green).
    b.put(0, 1);
    b.put(0, 1);
    b.put(1, 1);
    b.put(0, 3);
    b.put(0, 1);
    let last = groups - 1;
    for symbol in [last as u8, (last >> 8) as u8, 0, 0, 0] {
        b.simple(symbol);
    }
    for _ in 0..last {
        for _ in 0..5 {
            b.full();
        }
    }
    // The last group: black, opaque, read without a bit.
    for symbol in [0, 0, 0, 255, 0] {
        b.simple(symbol);
    }
    let mut data = b.out;
    data.extend([0; 8]);
    let mut riff = b"RIFF\0\0\0\0WEBPVP8L".to_vec();
    riff.extend((data.len() as u32).to_le_bytes());
    riff.extend(data);
    let len = riff.len() as u32 - 8;
    riff[4..8].copy_from_slice(&len.to_le_bytes());
    riff
}
