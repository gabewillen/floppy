//! A JPEG decoded in 8-bit grayscale at an eighth of each side, as libjpeg
//! decodes it for `OpenCV`'s reduced decode (`IMREAD_REDUCED_GRAYSCALE_8`,
//! libjpeg's `scale_denom = 8`).
//!
//! At that scale libjpeg's inverse DCT is `jpeg_idct_1x1`: each 8 × 8 block
//! of a component whose sampling factors are the frame's largest becomes
//! one pixel, `range_limit[(DC × q + 4) >> 3]`, its dequantized DC
//! coefficient over 8 plus 128 with libjpeg's range limiting. So this
//! module reads only what that needs: the markers, and the DC coefficients
//! of the entropy-coded data (Huffman, baseline, extended sequential or
//! progressive, with restart intervals), decoding and skipping the AC
//! coefficients of a sequential scan and skipping a progressive AC scan
//! whole. The pixel is libjpeg's to the bit.
//!
//! The gray is what `OpenCV` asks libjpeg for (`grfmt_jpeg.cpp`): a
//! one-component or YCbCr JPEG decoded straight to `JCS_GRAYSCALE` (the
//! luma), an RGB one reduced by libjpeg's `rgb_gray_convert`, and a CMYK or
//! YCCK one decoded to `JCS_CMYK` and reduced by `OpenCV`'s
//! `icvCvt_CMYK2Gray_8u_C4C1R`. libjpeg reads the color space from the
//! markers as `default_decompress_parms` does.
//!
//! What libjpeg refuses, this refuses (`None`): no frame or no scan, a
//! frame or scan header out of its bounds, a Huffman or quantization table
//! that is missing or malformed, a progression libjpeg calls bad, a marker
//! it does not know. So does what fails only because `OpenCV`'s memory
//! source cannot be refilled (its `fill_input_buffer` suspends, and the
//! read fails): entropy data that runs into the image's end, and an image
//! that ends without EOI after several scans. What libjpeg decodes past
//! with a warning, this decodes past the same way: entropy data that ends
//! early at a marker reads as zero bits and leaves the rest of its restart
//! interval zero, and a Huffman code that matches nothing reads as symbol
//! 0. A sequential JPEG without Huffman tables gets the standard ones
//! (Motion JPEG), as libjpeg-turbo gives them.
//!
//! Not decoded, where libjpeg would decode: arithmetic coding, and a
//! component the gray needs whose sampling factors are not the frame's
//! largest (subsampled luma, or subsampled RGB or CMYK), which libjpeg
//! reads through a larger inverse DCT that needs the AC coefficients.
//! Lossless, hierarchical and 12-bit JPEGs, which `OpenCV`'s reduced decode
//! does not read either, are not decoded.

use crate::{Gray, exif};

/// libjpeg's bound on the blocks of one interleaved MCU
/// (`D_MAX_BLOCKS_IN_MCU`).
const MAX_BLOCKS_IN_MCU: usize = 10;

/// libjpeg's bound on a frame's components (`MAX_COMPONENTS`).
const MAX_COMPONENTS: usize = 10;

/// libjpeg's bound on an image side (`JPEG_MAX_DIMENSION`).
const MAX_DIMENSION: usize = 65_500;

/// The standard Huffman tables (JPEG K.3, libjpeg-turbo's `jstdhuff.c`):
/// the counts of codes of each length 1 to 16, then the symbols.
const STANDARD_DC: [(&[u8; 16], &[u8]); 2] = [
    (
        &[0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    ),
    (
        &[0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    ),
];

/// The standard AC tables, as [`STANDARD_DC`].
const STANDARD_AC: [(&[u8; 16], &[u8]); 2] = [
    (
        &[0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d],
        &[
            0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51,
            0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1,
            0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
            0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57,
            0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75,
            0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92,
            0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
            0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
            0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8,
            0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    ),
    (
        &[0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77],
        &[
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07,
            0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09,
            0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25,
            0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
            0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56,
            0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74,
            0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
            0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba,
            0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6,
            0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    ),
];

/// A Huffman table as defined (`JHUFF_TBL`): the counts of codes of each
/// length, and the symbols, zero-filled to 256.
#[derive(Clone)]
struct Table {
    counts: [u8; 16],
    symbols: [u8; 256],
}

impl Table {
    fn new(counts: &[u8; 16], symbols: &[u8]) -> Self {
        let mut table = Self {
            counts: *counts,
            symbols: [0; 256],
        };
        for (slot, &symbol) in table.symbols.iter_mut().zip(symbols) {
            *slot = symbol;
        }
        table
    }
}

/// A Huffman table ready to decode (`d_derived_tbl`, libjpeg's
/// `jpeg_make_d_derived_tbl`): the largest code of each length (-1 for
/// none; the 17th ends every search) and the offset from a code to its
/// symbol.
struct Huffman {
    largest: [i64; 18],
    offset: [i64; 18],
    symbols: [u8; 256],
}

impl Huffman {
    /// The derived table, or `None` where libjpeg refuses it
    /// (`JERR_BAD_HUFF_TABLE`): over 256 codes, codes that overflow their
    /// lengths, or a DC symbol over 15.
    fn new(table: &Table, dc: bool) -> Option<Self> {
        let mut largest = [-1; 18];
        let mut offset = [0; 18];
        let mut code: i64 = 0;
        let mut symbol: i64 = 0;
        for (length, &count) in (1..=16).zip(&table.counts) {
            let count = i64::from(count);
            if count > 0 {
                *offset.get_mut(length)? = symbol - code;
                code += count;
                symbol += count;
                *largest.get_mut(length)? = code - 1;
                if symbol > 256 || code >= 1 << length {
                    return None;
                }
            }
            code <<= 1;
        }
        *largest.get_mut(17)? = 0xF_FFFF;
        let used = usize::try_from(symbol).ok()?;
        if dc && table.symbols.iter().take(used).any(|&symbol| symbol > 15) {
            return None;
        }
        Some(Self {
            largest,
            offset,
            symbols: table.symbols,
        })
    }
}

/// The entropy-coded bits of one restart interval, stuffing removed: past
/// the end they read as zeros and mark the interval exhausted, as libjpeg's
/// `jpeg_fill_bit_buffer` does at a marker.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    bit: u32,
    exhausted: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            at: 0,
            bit: 0,
            exhausted: false,
        }
    }

    fn bit(&mut self) -> i64 {
        let Some(&byte) = self.data.get(self.at) else {
            self.exhausted = true;
            return 0;
        };
        let value = (byte >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.at += 1;
        }
        i64::from(value)
    }

    fn bits(&mut self, count: u8) -> i64 {
        (0..count).fold(0, |value, _| (value << 1) | self.bit())
    }

    /// One symbol (`jpeg_huff_decode`): a code that matches nothing in 16
    /// bits reads as 0 (`JWRN_HUFF_BAD_CODE`).
    fn symbol(&mut self, table: &Huffman) -> u8 {
        let mut code = self.bit();
        let mut length = 1;
        while table
            .largest
            .get(length)
            .is_some_and(|&largest| code > largest)
        {
            code = (code << 1) | self.bit();
            length += 1;
        }
        if length > 16 {
            return 0;
        }
        table
            .offset
            .get(length)
            .and_then(|offset| usize::try_from(code + offset).ok())
            .and_then(|at| table.symbols.get(at))
            .copied()
            .unwrap_or(0)
    }

    /// A DC difference: its size category, then that many bits, extended
    /// to a signed value (`HUFF_EXTEND`).
    fn difference(&mut self, table: &Huffman) -> i64 {
        let size = self.symbol(table);
        if size == 0 {
            return 0;
        }
        let value = self.bits(size);
        if value < 1 << (size - 1) {
            value - (1 << size) + 1
        } else {
            value
        }
    }

    /// The AC coefficients of a sequential block, decoded and dropped.
    fn skip_ac(&mut self, table: &Huffman) {
        let mut at = 1;
        while at < 64 {
            let symbol = self.symbol(table);
            let (run, size) = (symbol >> 4, symbol & 15);
            if size != 0 {
                at += usize::from(run);
                self.bits(size);
            } else if run != 15 {
                break;
            } else {
                at += 15;
            }
            at += 1;
        }
    }
}

/// One frame component (`jpeg_component_info`) and its DC coefficients,
/// one per block of its MCU-padded grid.
struct Component {
    id: u8,
    horizontal: usize,
    vertical: usize,
    quantization: usize,
    /// The DC quantizer latched at the component's first scan
    /// (`latch_quant_tables`); none until then, which reads as 0.
    quantizer: Option<u16>,
    /// The grid's width in blocks.
    stride: usize,
    coefficients: Vec<i16>,
}

/// The frame (`SOFn`).
struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    horizontal: usize,
    vertical: usize,
    components: Vec<Component>,
}

impl Frame {
    /// MCUs across and down an interleaved scan.
    fn mcus(&self) -> (usize, usize) {
        (
            self.width.div_ceil(8 * self.horizontal),
            self.height.div_ceil(8 * self.vertical),
        )
    }
}

/// One component of a scan: its frame index and its DC and AC tables.
struct ScanComponent {
    index: usize,
    dc: usize,
    ac: usize,
}

/// A scan's header (`SOS`).
struct Scan {
    components: Vec<ScanComponent>,
    start: u8,
    end: u8,
    high: u8,
    low: u8,
}

/// How libjpeg reads a JPEG's components (`default_decompress_parms`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Color {
    /// One component, or YCbCr: the gray is the first component.
    Luma,
    /// Red, green and blue.
    Rgb,
    /// CMYK, as stored.
    Cmyk,
    /// YCCK, which libjpeg converts to CMYK.
    Ycck,
}

/// The decoder's state across the markers.
struct Decoder<'a> {
    image: &'a [u8],
    frame: Option<Frame>,
    dc_tables: [Option<Table>; 4],
    ac_tables: [Option<Table>; 4],
    quantizers: [Option<u16>; 4],
    restart_interval: usize,
    jfif: bool,
    adobe: Option<u8>,
    orientation: Option<u16>,
    scanned: bool,
    /// Whether libjpeg reads every scan before its output
    /// (`has_multiple_scans`: progressive, or a first scan without every
    /// component).
    buffered: bool,
}

/// The JPEG at an eighth of each side in 8-bit grayscale, and the
/// orientation its first Exif segment before the first scan names (see
/// the module documentation).
pub fn reduced(image: &[u8]) -> Option<(Gray, Option<u16>)> {
    let mut decoder = Decoder {
        image,
        frame: None,
        dc_tables: Default::default(),
        ac_tables: Default::default(),
        quantizers: [None; 4],
        restart_interval: 0,
        jfif: false,
        adobe: None,
        orientation: None,
        scanned: false,
        buffered: false,
    };
    decoder.markers()?;
    let gray = decoder.gray()?;
    Some((gray, decoder.orientation))
}

impl Decoder<'_> {
    /// Reads every marker to the end of the image (`read_markers`): a
    /// marker is `0xff`, any fill bytes, then its code; bytes before it
    /// are skipped, as libjpeg's `next_marker` skips them.
    fn markers(&mut self) -> Option<()> {
        if self.image.get(..2) != Some([0xff, 0xd8].as_slice()) {
            return None;
        }
        let mut at = 2;
        loop {
            // `next_marker`: skip to an 0xff, then its fill bytes; 0xff 0x00
            // is not a marker. At the image's end without EOI a single
            // scan has been read out already, but several are still being
            // buffered, so the read fails.
            let code = loop {
                while self.image.get(at).is_some_and(|&byte| byte != 0xff) {
                    at += 1;
                }
                while self.image.get(at) == Some(&0xff) {
                    at += 1;
                }
                let Some(&code) = self.image.get(at) else {
                    return (!self.buffered).then_some(());
                };
                at += 1;
                if code != 0 {
                    break code;
                }
            };
            match code {
                // EOI, or a restart or TEM marker out of a scan (ignored).
                0xd9 => return Some(()),
                0xd0..=0xd7 | 0x01 => continue,
                _ => {}
            }
            let length = usize::from(u16::from_be_bytes([
                *self.image.get(at)?,
                *self.image.get(at + 1)?,
            ]));
            let segment = self.image.get(at + 2..at.checked_add(length)?)?;
            at += length;
            match code {
                0xc0..=0xc2 => self.frame(segment, code == 0xc2)?,
                0xc4 => self.huffman_tables(segment)?,
                0xdb => self.quantization_tables(segment)?,
                0xdd => {
                    let [high, low] = segment else {
                        return None;
                    };
                    self.restart_interval = usize::from(u16::from_be_bytes([*high, *low]));
                }
                0xe0 => self.jfif |= segment.len() >= 14 && segment.starts_with(b"JFIF\0"),
                0xe1 if !self.scanned && segment.len() > 6 && segment.starts_with(b"Exif\0\0") => {
                    if self.orientation.is_none() {
                        self.orientation = exif::orientation(segment.get(6..)?);
                    }
                }
                0xee if segment.len() >= 12 && segment.starts_with(b"Adobe") => {
                    self.adobe = segment.get(11).copied();
                }
                0xda => at = self.scan(segment, at)?,
                // DAC, DNL, the other application segments and comments.
                0xcc | 0xdc | 0xe0..=0xef | 0xfe => {}
                // Lossless, hierarchical or arithmetic coded frames, a
                // second SOI, and the markers libjpeg does not know
                // (`JERR_UNKNOWN_MARKER`).
                _ => return None,
            }
        }
    }

    /// A frame header (`get_sof`).
    fn frame(&mut self, segment: &[u8], progressive: bool) -> Option<()> {
        if self.frame.is_some() {
            return None;
        }
        let [
            precision,
            height_high,
            height_low,
            width_high,
            width_low,
            count,
            rest @ ..,
        ] = segment
        else {
            return None;
        };
        let height = usize::from(u16::from_be_bytes([*height_high, *height_low]));
        let width = usize::from(u16::from_be_bytes([*width_high, *width_low]));
        let count = usize::from(*count);
        if *precision != 8
            || width == 0
            || height == 0
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || !crate::bounded(width, height)
            || count == 0
            || count > MAX_COMPONENTS
            || rest.len() != 3 * count
        {
            return None;
        }
        let mut components: Vec<Component> = rest
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[id, sampling, quantization]| Component {
                id,
                horizontal: usize::from(sampling >> 4),
                vertical: usize::from(sampling & 15),
                quantization: usize::from(quantization),
                quantizer: None,
                stride: 0,
                coefficients: Vec::new(),
            })
            .collect();
        if components.iter().any(|component| {
            !(1..=4).contains(&component.horizontal)
                || !(1..=4).contains(&component.vertical)
                || component.quantization > 3
        }) {
            return None;
        }
        let horizontal = components
            .iter()
            .map(|component| component.horizontal)
            .max()?;
        let vertical = components
            .iter()
            .map(|component| component.vertical)
            .max()?;
        let (across, down) = (
            width.div_ceil(8 * horizontal),
            height.div_ceil(8 * vertical),
        );
        for component in &mut components {
            component.stride = across * component.horizontal;
            component.coefficients = vec![0; component.stride * down * component.vertical];
        }
        self.frame = Some(Frame {
            width,
            height,
            progressive,
            horizontal,
            vertical,
            components,
        });
        Some(())
    }

    /// Huffman tables (`get_dht`).
    fn huffman_tables(&mut self, mut segment: &[u8]) -> Option<()> {
        while let [class, counts @ ..] = segment {
            let counts: &[u8; 16] = counts.first_chunk()?;
            let total = counts
                .iter()
                .map(|&count| usize::from(count))
                .sum::<usize>();
            let symbols = segment.get(17..17 + total)?;
            if total > 256 {
                return None;
            }
            let table = Some(Table::new(counts, symbols));
            match *class {
                0..=3 => *self.dc_tables.get_mut(usize::from(*class))? = table,
                0x10..=0x13 => *self.ac_tables.get_mut(usize::from(class - 0x10))? = table,
                _ => return None,
            }
            segment = segment.get(17 + total..)?;
        }
        Some(())
    }

    /// Quantization tables (`get_dqt`): only each table's DC quantizer, its
    /// first entry, is kept.
    fn quantization_tables(&mut self, mut segment: &[u8]) -> Option<()> {
        while let [selector, entries @ ..] = segment {
            let (precision, index) = (selector >> 4, usize::from(selector & 15));
            if index > 3 {
                return None;
            }
            // A short table is read as far as it goes, the rest of it 1.
            let size = if precision == 0 { 1 } else { 2 };
            let count = (entries.len() / size).min(64);
            let quantizer = match entries {
                _ if count == 0 => 1,
                [high, low, ..] if size == 2 => u16::from_be_bytes([*high, *low]),
                [value, ..] => u16::from(*value),
                [] => 1,
            };
            *self.quantizers.get_mut(index)? = Some(quantizer);
            segment = entries.get(count * size..)?;
        }
        Some(())
    }

    /// A scan: its header, then its entropy-coded data from `at`; the
    /// offset of the marker that ends it.
    fn scan(&mut self, segment: &[u8], at: usize) -> Option<usize> {
        let scan = self.scan_header(segment)?;
        let (intervals, end) = intervals(self.image, at);
        if end >= self.image.len() {
            return None;
        }
        // The first scan installs the standard tables in the empty slots
        // of a sequential frame (`jinit_huff_decoder`).
        let frame = self.frame.as_mut()?;
        if !self.scanned {
            self.buffered = frame.progressive || scan.components.len() < frame.components.len();
        }
        if !self.scanned && !frame.progressive {
            for (slot, (counts, symbols)) in self.dc_tables.iter_mut().zip(STANDARD_DC) {
                slot.get_or_insert_with(|| Table::new(counts, symbols));
            }
            for (slot, (counts, symbols)) in self.ac_tables.iter_mut().zip(STANDARD_AC) {
                slot.get_or_insert_with(|| Table::new(counts, symbols));
            }
        }
        self.scanned = true;
        // Each component latches its quantizer at its first scan.
        for component in &scan.components {
            let component = frame.components.get_mut(component.index)?;
            if component.quantizer.is_none() {
                component.quantizer = Some((*self.quantizers.get(component.quantization)?)?);
            }
        }
        let table = |tables: &[Option<Table>; 4], index: usize, dc: bool| {
            tables
                .get(index)?
                .as_ref()
                .and_then(|table| Huffman::new(table, dc))
        };
        let dc_band = scan.start == 0;
        let tables = scan
            .components
            .iter()
            .map(|component| {
                let dc = if frame.progressive && !(dc_band && scan.high == 0) {
                    None
                } else {
                    Some(table(&self.dc_tables, component.dc, true)?)
                };
                let ac = if frame.progressive && dc_band {
                    None
                } else {
                    Some(table(&self.ac_tables, component.ac, false)?)
                };
                Some((dc, ac))
            })
            .collect::<Option<Vec<_>>>()?;
        if frame.progressive {
            // `start_pass_phuff_decoder`'s bad progressions.
            let bad = if dc_band {
                scan.end != 0
            } else {
                scan.start > scan.end || scan.end > 63 || scan.components.len() != 1
            } || (scan.high != 0 && scan.low + 1 != scan.high)
                || scan.low > 13;
            if bad {
                return None;
            }
            if !dc_band {
                return Some(end);
            }
        }
        decode_scan(frame, &scan, &tables, &intervals, self.restart_interval)?;
        Some(end)
    }

    /// A scan header (`get_sos`).
    fn scan_header(&self, segment: &[u8]) -> Option<Scan> {
        let frame = self.frame.as_ref()?;
        let [count, rest @ ..] = segment else {
            return None;
        };
        let count = usize::from(*count);
        if !(1..=4).contains(&count) || rest.len() != 2 * count + 3 {
            return None;
        }
        let mut components: Vec<ScanComponent> = Vec::with_capacity(count);
        for selector in rest.as_chunks::<2>().0.iter().take(count) {
            let [id, tables] = *selector;
            let index = frame
                .components
                .iter()
                .position(|component| component.id == id)?;
            if components.iter().any(|component| component.index == index) {
                return None;
            }
            components.push(ScanComponent {
                index,
                dc: usize::from(tables >> 4),
                ac: usize::from(tables & 15),
            });
        }
        let [start, end, approximation] = *rest.get(2 * count..)? else {
            return None;
        };
        if count > 1 {
            let blocks: usize = components
                .iter()
                .filter_map(|component| frame.components.get(component.index))
                .map(|component| component.horizontal * component.vertical)
                .sum();
            if blocks > MAX_BLOCKS_IN_MCU {
                return None;
            }
        }
        Some(Scan {
            components,
            start,
            end,
            high: approximation >> 4,
            low: approximation & 15,
        })
    }

    /// The color space libjpeg reads (`default_decompress_parms`); `None`
    /// for a count of components it cannot reduce to gray
    /// (`JERR_CONVERSION_NOTIMPL`).
    fn color(&self, frame: &Frame) -> Option<Color> {
        let ids: Vec<u8> = frame
            .components
            .iter()
            .map(|component| component.id)
            .collect();
        Some(match (frame.components.len(), self.adobe) {
            (3, Some(0)) if !self.jfif => Color::Rgb,
            (3, None) if !self.jfif && ids == b"RGB" => Color::Rgb,
            (1 | 3, _) => Color::Luma,
            (4, None | Some(0)) => Color::Cmyk,
            (4, Some(_)) => Color::Ycck,
            _ => return None,
        })
    }

    /// The reduced gray of the decoded coefficients.
    fn gray(&self) -> Option<Gray> {
        let frame = self.frame.as_ref()?;
        if !self.scanned {
            return None;
        }
        let color = self.color(frame)?;
        let needed = if color == Color::Luma {
            frame.components.get(..1)?
        } else {
            &frame.components[..]
        };
        // A needed component at the frame's largest sampling factors is
        // read through `jpeg_idct_1x1`; any other is not decoded here.
        if needed.iter().any(|component| {
            component.horizontal != frame.horizontal || component.vertical != frame.vertical
        }) {
            return None;
        }
        let (width, height) = (frame.width.div_ceil(8), frame.height.div_ceil(8));
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let sample = |component: &Component| -> Option<i32> {
                    let coefficient = *component.coefficients.get(y * component.stride + x)?;
                    // `DEQUANTIZE` in `MULTIPLIER` (a `short`), then
                    // `DESCALE(dc, 3)` and the range limit.
                    let [high, low] = component.quantizer.unwrap_or(0).to_be_bytes();
                    let quantizer = i32::from(i16::from_be_bytes([high, low]));
                    Some(i32::from(range_limit(
                        (i32::from(coefficient) * quantizer + 4) >> 3,
                    )))
                };
                let samples = needed.iter().map(sample).collect::<Option<Vec<_>>>()?;
                pixels.push(match (color, samples.as_slice()) {
                    (Color::Luma, [luma]) => u8::try_from(*luma).ok()?,
                    (Color::Rgb, &[red, green, blue]) => rgb_gray(red, green, blue)?,
                    (Color::Cmyk, &[cyan, magenta, yellow, key]) => {
                        cmyk_gray(cyan, magenta, yellow, key)?
                    }
                    (Color::Ycck, &[luma, blue, red, key]) => {
                        let [cyan, magenta, yellow] = ycc_cmy(luma, blue, red);
                        cmyk_gray(cyan, magenta, yellow, key)?
                    }
                    _ => return None,
                });
            }
        }
        Some(Gray {
            width,
            height,
            pixels,
        })
    }
}

/// The entropy-coded data of a scan from `at`, split at its restart
/// markers, stuffing removed; and the offset of the marker that ends it
/// (or the image's end). As libjpeg reads it, `0xff` and any fill bytes
/// then `0x00` is a data `0xff`, then a restart code a new interval, then
/// any other code the end.
fn intervals(image: &[u8], mut at: usize) -> (Vec<Vec<u8>>, usize) {
    let mut intervals = vec![Vec::new()];
    loop {
        let Some(&byte) = image.get(at) else {
            return (intervals, at);
        };
        if byte != 0xff {
            if let Some(interval) = intervals.last_mut() {
                interval.push(byte);
            }
            at += 1;
            continue;
        }
        let mut code_at = at + 1;
        while image.get(code_at) == Some(&0xff) {
            code_at += 1;
        }
        match image.get(code_at) {
            Some(0) => {
                if let Some(interval) = intervals.last_mut() {
                    interval.push(0xff);
                }
                at = code_at + 1;
            }
            Some(0xd0..=0xd7) => {
                intervals.push(Vec::new());
                at = code_at + 1;
            }
            _ => return (intervals, at),
        }
    }
}

/// Decodes a sequential scan, or a progressive DC scan, into the frame's
/// DC coefficients (`decode_mcu`, `decode_mcu_DC_first`,
/// `decode_mcu_DC_refine`). Each restart interval resets the DC
/// predictions; an interval whose data ran out leaves its remaining MCUs
/// as they were (zero), until a restart marker that was there.
fn decode_scan(
    frame: &mut Frame,
    scan: &Scan,
    tables: &[(Option<Huffman>, Option<Huffman>)],
    intervals: &[Vec<u8>],
    restart_interval: usize,
) -> Option<()> {
    let empty = Vec::new();
    // The blocks of one MCU: (scan component, column, row) offsets.
    let interleaved = scan.components.len() > 1;
    let (across, down) = if interleaved {
        frame.mcus()
    } else {
        let component = frame.components.get(scan.components.first()?.index)?;
        (
            (frame.width * component.horizontal).div_ceil(8 * frame.horizontal),
            (frame.height * component.vertical).div_ceil(8 * frame.vertical),
        )
    };
    let mut blocks = Vec::new();
    for (position, selector) in scan.components.iter().enumerate() {
        let component = frame.components.get(selector.index)?;
        let (columns, rows) = if interleaved {
            (component.horizontal, component.vertical)
        } else {
            (1, 1)
        };
        for row in 0..rows {
            for column in 0..columns {
                blocks.push((position, column, row));
            }
        }
    }
    let mut interval = 0;
    let mut bits = Bits::new(intervals.first().unwrap_or(&empty));
    let mut insufficient = false;
    let mut predictions = vec![0_i64; scan.components.len()];
    for mcu in 0..across * down {
        if restart_interval > 0 && mcu > 0 && mcu % restart_interval == 0 {
            interval += 1;
            predictions.fill(0);
            if let Some(data) = intervals.get(interval) {
                bits = Bits::new(data);
                insufficient = false;
            } else {
                bits = Bits::new(&empty);
            }
        }
        if insufficient {
            continue;
        }
        let (mcu_x, mcu_y) = (mcu % across, mcu / across);
        for &(position, column, row) in &blocks {
            let selector = scan.components.get(position)?;
            let component = frame.components.get_mut(selector.index)?;
            let (horizontal, vertical) = if interleaved {
                (component.horizontal, component.vertical)
            } else {
                (1, 1)
            };
            let block = (mcu_y * vertical + row) * component.stride + mcu_x * horizontal + column;
            let (dc, ac) = tables.get(position)?;
            let prediction = predictions.get_mut(position)?;
            let coefficient = component.coefficients.get_mut(block)?;
            if !frame.progressive {
                let difference = bits.difference(dc.as_ref()?);
                // libjpeg sums in unsigned `int` and stores a `JCOEF`.
                *prediction = i64::from(wrapped(*prediction + difference));
                *coefficient = short(*prediction);
                bits.skip_ac(ac.as_ref()?);
            } else if scan.high == 0 {
                let difference = bits.difference(dc.as_ref()?);
                let sum = *prediction + difference;
                // `JERR_BAD_DCT_COEF`.
                if i32::try_from(sum).is_err() {
                    return None;
                }
                *prediction = sum;
                *coefficient = short(i64::from(wrapped(sum << scan.low)));
            } else if bits.bit() == 1 {
                *coefficient |= short(1 << scan.low);
            }
        }
        insufficient = bits.exhausted;
    }
    Some(())
}

/// A sum wrapped to a C `int`.
fn wrapped(value: i64) -> i32 {
    let [.., a, b, c, d] = value.to_be_bytes();
    i32::from_be_bytes([a, b, c, d])
}

/// A value cut to a `JCOEF` (`short`).
fn short(value: i64) -> i16 {
    let [.., high, low] = value.to_be_bytes();
    i16::from_be_bytes([high, low])
}

/// libjpeg's post-IDCT range limit (`IDCT_range_limit`, indexed by
/// `value & RANGE_MASK`): -128..127 is that plus 128, then 255 up to 511,
/// 0 to 895, and the wrapped negatives again.
fn range_limit(value: i32) -> u8 {
    let masked = value & 1023;
    let limited = match masked {
        0..=127 => masked + 128,
        128..=511 => 255,
        512..=895 => 0,
        _ => masked - 896,
    };
    u8::try_from(limited).unwrap_or(0)
}

/// libjpeg's `rgb_gray_convert`: 0.299, 0.587 and 0.114 in 1/65536ths,
/// rounded (`FIX`), half added.
fn rgb_gray(red: i32, green: i32, blue: i32) -> Option<u8> {
    u8::try_from((19595 * red + 38470 * green + 7471 * blue + 32768) >> 16).ok()
}

/// libjpeg's `ycck_cmyk_convert` of the first three components: each is
/// 255 less the color its YCbCr gives, clamped (`build_ycc_rgb_table`).
fn ycc_cmy(luma: i32, blue: i32, red: i32) -> [i32; 3] {
    let (blue, red) = (i64::from(blue - 128), i64::from(red - 128));
    let fix = |factor: f64| {
        #[allow(clippy::cast_possible_truncation, reason = "libjpeg's FIX, below 2^17")]
        let fixed = (factor * 65536.0 + 0.5) as i64;
        fixed
    };
    let half = 1 << 15;
    let red_r = (fix(1.402) * red + half) >> 16;
    let blue_b = (fix(1.772) * blue + half) >> 16;
    let green = (-fix(0.344_14) * blue + half - fix(0.714_14) * red) >> 16;
    let clamp = |value: i64| i32::try_from(value.clamp(0, 255)).unwrap_or(0);
    let luma = i64::from(luma);
    [
        clamp(255 - (luma + red_r)),
        clamp(255 - (luma + green)),
        clamp(255 - (luma + blue_b)),
    ]
}

/// `OpenCV`'s `icvCvt_CMYK2Gray_8u_C4C1R`: each ink darkened by the key,
/// then its BT.601 weights (cyan as red, yellow as blue) in 1/16384ths,
/// rounded.
fn cmyk_gray(cyan: i32, magenta: i32, yellow: i32, key: i32) -> Option<u8> {
    let ink = |value: i32| key - (((255 - value) * key) >> 8);
    let [r, g, b] = crate::CV_GRAY.map(|weight| i32::try_from(weight).unwrap_or(0));
    u8::try_from((ink(yellow) * b + ink(magenta) * g + ink(cyan) * r + 8192) >> 14).ok()
}
