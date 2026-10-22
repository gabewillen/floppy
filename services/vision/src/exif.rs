//! The Exif orientation `OpenCV` turns a decoded image by
//! (`ApplyExifOrientation` after `imdecode`, unless
//! `IMREAD_IGNORE_ORIENTATION`), which the original's reduced decode does.
//! `OpenCV` reads it from a JPEG's `Exif` APP1 segment and a PNG's `eXIf`
//! chunk (a WebP's only when metadata is asked for, which the original
//! does not), with its own TIFF reader (`ExifReader`), whose quirks this
//! keeps: it reads the first directory's entries in order, keeps the first
//! orientation, and stops at the first entry it cannot read.

use crate::Gray;

/// TIFF's byte order mark value (`tagMarkRequired`).
const TIFF_MARK: u16 = 0x2a;

/// The orientation tag (`ORIENTATION`).
const ORIENTATION: u16 = 0x0112;

/// How the TIFF data orders its bytes (`Endianness_t`): `II` little,
/// anything else big.
#[derive(Clone, Copy)]
struct Reader<'a> {
    data: &'a [u8],
    little: bool,
}

impl Reader<'_> {
    /// `getU16`: `None` past the end (`ExifParsingError`).
    fn u16(self, at: usize) -> Option<u16> {
        let bytes = [*self.data.get(at)?, *self.data.get(at.checked_add(1)?)?];
        Some(if self.little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    }

    /// `getU32`.
    fn u32(self, at: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    /// `getURational`, read only for its bounds.
    fn rationals(self, at: u32, count: u32) -> Option<()> {
        for index in 0..count {
            let at = usize::try_from(at.wrapping_add(8 * index)).ok()?;
            self.u32(at)?;
            self.u32(at.checked_add(4)?)?;
        }
        Some(())
    }

    /// `getString`, read only for its bounds: its count, then its data at
    /// offset 8 of the TIFF data when it fits in 4 bytes (`OpenCV`'s
    /// reading), else at the offset the entry names.
    fn string(self, entry: usize) -> Option<()> {
        let size = usize::try_from(self.u32(entry.checked_add(4)?)?).ok()?;
        let offset = if size > 4 {
            usize::try_from(self.u32(entry.checked_add(8)?)?).ok()?
        } else {
            8
        };
        (offset <= self.data.len() && offset.checked_add(size)? <= self.data.len()).then_some(())
    }
}

/// The orientation the TIFF data of an Exif segment names, if `OpenCV`
/// reads one.
pub fn orientation(data: &[u8]) -> Option<u16> {
    let little = match data {
        [first, second, ..] if first != second => false,
        [first, ..] => *first == b'I',
        [] => return None,
    };
    let reader = Reader { data, little };
    if reader.u16(2)? != TIFF_MARK {
        return None;
    }
    let mut entry = usize::try_from(reader.u32(4)?).ok()?;
    let count = reader.u16(entry)?;
    entry = entry.checked_add(2)?;
    for _ in 0..count {
        let value = entry.checked_add(8)?;
        match reader.u16(entry)? {
            ORIENTATION => return reader.u16(value),
            // Strings: the description, make, model, software, date and
            // copyright.
            0x010e | 0x010f | 0x0110 | 0x0131 | 0x0132 | 0x8298 => reader.string(entry)?,
            // Rationals: the resolutions, the white point, the primaries,
            // the YCbCr coefficients, the reference black and white.
            0x011a | 0x011b => reader.rationals(reader.u32(value)?, 1)?,
            0x013e => reader.rationals(reader.u32(value)?, 2)?,
            0x013f | 0x0214 => reader.rationals(reader.u32(value)?, 6)?,
            0x0211 => reader.rationals(reader.u32(value)?, 3)?,
            // The resolution unit and the YCbCr positioning.
            0x0128 | 0x0213 => {
                reader.u16(value)?;
            }
            _ => {}
        }
        entry = entry.checked_add(12)?;
    }
    None
}

/// An image turned by an Exif orientation as `OpenCV`'s `ExifTransform`
/// turns it: 2 mirrors it, 3 turns it half round, 4 flips it, 5 transposes
/// it, 6 to 8 transpose it then mirror, turn or flip it, and any other
/// value leaves it.
pub fn oriented(gray: Gray, orientation: Option<u16>) -> Gray {
    let Some(orientation @ 2..=8) = orientation else {
        return gray;
    };
    let transposed = orientation >= 5;
    let (width, height) = if transposed {
        (gray.height, gray.width)
    } else {
        (gray.width, gray.height)
    };
    // After the transpose: 2 and 6 mirror, 3 and 7 turn, 4 and 8 flip.
    let (mirror, flip) = match orientation {
        2 | 6 => (true, false),
        3 | 7 => (true, true),
        4 | 8 => (false, true),
        _ => (false, false),
    };
    let mut pixels = Vec::with_capacity(gray.pixels.len());
    for y in 0..height {
        for x in 0..width {
            let x = if mirror { width - 1 - x } else { x };
            let y = if flip { height - 1 - y } else { y };
            let (source_x, source_y) = if transposed { (y, x) } else { (x, y) };
            pixels.push(
                gray.pixels
                    .get(source_y * gray.width + source_x)
                    .copied()
                    .unwrap_or(0),
            );
        }
    }
    Gray {
        width,
        height,
        pixels,
    }
}
