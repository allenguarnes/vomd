use super::*;

/// A TIFF block of the byte order `big` whose first IFD (at `ifd`) has `entries`: tag, type,
/// count and value (the value's first two bytes, as a SHORT is stored).
fn tiff(big: bool, ifd: u32, count: u16, entries: &[(u16, u16, u32, u16)]) -> Vec<u8> {
    let u16b = |v: u16| {
        if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut b = if big {
        b"MM\0*".to_vec()
    } else {
        b"II*\0".to_vec()
    };
    b.extend(u32b(ifd));
    b.resize(ifd as usize, 0);
    b.extend(u16b(count));
    for &(tag, ty, n, value) in entries {
        b.extend(u16b(tag));
        b.extend(u16b(ty));
        b.extend(u32b(n));
        b.extend(u16b(value));
        b.extend([0, 0]);
    }
    b.extend(u32b(0));
    b
}

#[test]
fn the_orientation_is_read_in_either_byte_order() {
    for big in [false, true] {
        for o in 1..=8 {
            let other = (0x0110, 2, 4, 0);
            let b = tiff(big, 8, 2, &[other, (0x0112, 3, 1, o)]);
            assert_eq!(orientation(&b), o as u8, "big {big}");
            let mut exif = b"Exif\0\0".to_vec();
            exif.extend(&b);
            assert_eq!(orientation(&exif), o as u8, "big {big}, with its prefix");
        }
        // The IFD further on.
        assert_eq!(orientation(&tiff(big, 100, 1, &[(0x0112, 3, 1, 6)])), 6);
    }
}

#[test]
fn anything_but_a_short_of_1_to_8_is_as_stored() {
    for (ty, n, value) in [
        (3, 1, 0),
        (3, 1, 9),
        (3, 1, 0xFFFF),
        (4, 1, 6),
        (3, 2, 6),
        (3, 0, 6),
    ] {
        let b = tiff(false, 8, 1, &[(0x0112, ty, n, value)]);
        assert_eq!(orientation(&b), 1, "type {ty}, count {n}, value {value}");
    }
    // No orientation tag, not TIFF, nothing.
    assert_eq!(orientation(&tiff(true, 8, 1, &[(0x0110, 3, 1, 6)])), 1);
    for junk in [
        &b""[..],
        b"Exif\0\0",
        b"II*",
        b"II+\0\x08\0\0\0",
        b"MM*\0\0\0\0\x08",
    ] {
        assert_eq!(orientation(junk), 1, "{junk:?}");
    }
}

/// Offsets and counts that point past the block, or wrap around, read nothing past it.
#[test]
fn adversarial_offsets_and_counts_read_nothing_outside() {
    let good = tiff(false, 8, 1, &[(0x0112, 3, 1, 6)]);
    for ifd in [
        u32::MAX,
        u32::MAX - 1,
        0xFFFF_FFF0,
        good.len() as u32,
        good.len() as u32 - 1,
    ] {
        let mut b = good.clone();
        b[4..8].copy_from_slice(&ifd.to_le_bytes());
        assert_eq!(orientation(&b), 1, "IFD at {ifd}");
    }
    // A count far larger than the entries there are: those within the block are read.
    let mut b = good.clone();
    b[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(orientation(&b), 6);
    // The entry cut short at every length.
    for cut in 0..good.len() {
        let o = orientation(&good[..cut]);
        assert!(o == 1 || o == 6, "cut at {cut}: {o}");
    }
    // The IFD pointing at the header itself (as a loop of IFDs would): the bytes there are no
    // orientation.
    let mut b = good.clone();
    b[4..8].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(orientation(&b), 1);
}

#[test]
fn the_app1_put_back_says_its_orientation() {
    for o in 1..=8 {
        let s = app1(o);
        assert_eq!(s[..2], [0xFF, 0xE1]);
        assert_eq!(usize::from(u16::from_be_bytes([s[2], s[3]])), s.len() - 2);
        assert_eq!(orientation(&s[4..]), o);
    }
}
