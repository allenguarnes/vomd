//! The orientation in an image's EXIF data: tag `0x0112` of the first IFD of a TIFF block, the
//! form EXIF takes in a JPEG's `APP1`, a PNG's `eXIf` and a WebP's `EXIF` chunk. Nothing else of
//! it is read, and nothing is allocated.

/// What [`orientation`] is given of an `APP1` segment, at most: the first IFD comes right after
/// the TIFF header in what cameras and editors write.
pub const HEAD: usize = 64 << 10;

/// The orientation `tiff` says, 1 to 8 (see [`Header::orientation`](super::Header)); 1, as
/// stored, for anything else. `tiff` is a TIFF block (`II*\0` or `MM\0*`), with or without the
/// `Exif\0\0` that comes before it in a JPEG. Only the first IFD is read, and only its entries
/// within `tiff`: an offset or count past the end, or a loop of IFDs, is no orientation.
pub fn orientation(tiff: &[u8]) -> u8 {
    read(tiff).unwrap_or(1)
}

fn read(b: &[u8]) -> Option<u8> {
    let b = b.strip_prefix(b"Exif\0\0").unwrap_or(b);
    let big = match b.get(..4)? {
        b"II*\0" => false,
        b"MM\0*" => true,
        _ => return None,
    };
    let u16_at = |i: usize| {
        let s = b.get(i..i.checked_add(2)?)?;
        let s = [s[0], s[1]];
        Some(if big {
            u16::from_be_bytes(s)
        } else {
            u16::from_le_bytes(s)
        })
    };
    let u32_at = |i: usize| {
        let s = b.get(i..i.checked_add(4)?)?;
        let s = [s[0], s[1], s[2], s[3]];
        Some(if big {
            u32::from_be_bytes(s)
        } else {
            u32::from_le_bytes(s)
        })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    // Entries of 12 bytes after the count, as many as there is room for.
    let count = usize::from(u16_at(ifd)?).min((b.len() - ifd - 2) / 12);
    for i in 0..count {
        let entry = ifd + 2 + 12 * i;
        if u16_at(entry)? == 0x0112 {
            // A SHORT (3), one of them: its value is the first two bytes of the value field.
            if u16_at(entry + 2)? != 3 || u32_at(entry + 4)? != 1 {
                return None;
            }
            let value = u16_at(entry + 8)?;
            return (1..=8).contains(&value).then_some(value as u8);
        }
    }
    None
}

/// The bytes of [`app1`].
pub const APP1_LEN: usize = 36;

/// A JPEG `APP1` segment (marker included) of EXIF data that holds `orientation` and nothing
/// else: what is put back of a JPEG's EXIF when its metadata is cut out (`shed_jpeg_metadata`).
pub fn app1(orientation: u8) -> [u8; APP1_LEN] {
    let mut s = [0u8; APP1_LEN];
    s[..4].copy_from_slice(&[0xFF, 0xE1, 0, APP1_LEN as u8 - 2]);
    s[4..10].copy_from_slice(b"Exif\0\0");
    // The TIFF header (big-endian, the first IFD at 8), one entry, no next IFD.
    s[10..18].copy_from_slice(b"MM\0*\0\0\0\x08");
    s[18..20].copy_from_slice(&[0, 1]);
    s[20..32].copy_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
    s
}

#[cfg(test)]
mod tests;
