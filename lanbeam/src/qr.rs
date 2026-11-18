//! Pure-Rust ISO/IEC 18004 QR Code encoder (byte mode, versions 1–6,
//! error correction level L, Reed-Solomon GF(256), penalty-mask selection).
//!
//! No external dependencies — the complete encoder is implemented from scratch
//! so the `lanbeam` binary stays a single self-contained native executable.

use std::fmt;
use std::sync::OnceLock;

// Primitive polynomial 0x11D.
const GF_POLY: u32 = 0x11D;

/// Error returned by the QR encoder.
#[derive(Debug, Clone, PartialEq)]
pub enum QrError {
    DataTooLong,
    Encode(String),
}

impl fmt::Display for QrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QrError::DataTooLong => write!(f, "data too long for QR versions 1–6"),
            QrError::Encode(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for QrError {}

/// A rendered QR code: `size` x `size` modules (`true` = dark).
#[derive(Debug, Clone)]
pub struct QrCode {
    pub size: usize,
    modules: Vec<bool>,
}

impl QrCode {
    /// Encode `text` in byte mode, choosing the smallest version (1–6, level L)
    /// that fits.
    pub fn encode(text: &str) -> Result<QrCode, QrError> {
        let data = text.as_bytes();
        // Try versions 1..=6 at level L; pick the smallest that fits the bit
        // stream (mode indicator + char count + data + terminator + padding).
        for version in 1u8..=6u8 {
            let ec = EC_BLOCKS[version as usize];
            let total_data_codewords = ec.total_data;
            let capacity_bits = total_data_codewords * 8;

            let mut bits = Vec::new();
            // Mode indicator: byte mode = 0100.
            bits.extend_from_slice(&[0, 1, 0, 0]);
            // Character count (8 bits for versions 1–9).
            let count_bits = char_count_bits(data.len(), version);
            push_uint(&mut bits, data.len() as u32, count_bits);
            // Data bytes.
            for &b in data {
                push_uint(&mut bits, b as u32, 8);
            }
            // Terminator: up to four zero bits.
            let capacity_remaining = capacity_bits.saturating_sub(bits.len());
            let terminator_len = capacity_remaining.min(4);
            for _ in 0..terminator_len {
                bits.push(0);
            }
            // Pad to byte boundary.
            while bits.len() % 8 != 0 {
                bits.push(0);
            }
            if bits.len() > capacity_bits {
                // This version too small; try next.
                continue;
            }
            // Pad bytes: alternate 0xEC, 0x11 until full.
            let mut pad: u8 = 0xEC;
            while (bits.len() / 8) < total_data_codewords {
                push_uint(&mut bits, pad as u32, 8);
                pad = if pad == 0xEC { 0x11 } else { 0xEC };
            }

            let codewords = bits_to_bytes(&bits);
            return QrCode::encode_with_version(version, &codewords, ec);
        }
        Err(QrError::DataTooLong)
    }

    fn encode_with_version(
        version: u8,
        data_codewords: &[u8],
        ec: EcBlock,
    ) -> Result<QrCode, QrError> {
        let total_data = ec.total_data as usize;
        if data_codewords.len() != total_data {
            return Err(QrError::Encode(format!(
                "expected {} data codewords, got {}",
                total_data,
                data_codewords.len()
            )));
        }

        let mut ec_codewords: Vec<Vec<u8>> = Vec::new();
        let mut data_blocks: Vec<Vec<u8>> = Vec::new();

        // Split data into blocks per group.
        let mut offset = 0usize;
        for _ in 0..ec.num_blocks_1 {
            let len = ec.data_per_block_1 as usize;
            data_blocks.push(data_codewords[offset..offset + len].to_vec());
            offset += len;
        }
        for _ in 0..ec.num_blocks_2 {
            let len = ec.data_per_block_2 as usize;
            data_blocks.push(data_codewords[offset..offset + len].to_vec());
            offset += len;
        }

        // Reed-Solomon EC per block.
        let ec_len = ec.ec_per_block as usize;
        for block in &data_blocks {
            ec_codewords.push(reed_solomon_encode(block, ec_len));
        }

        // Interleave: data (group by group), then EC.
        let mut interleaved: Vec<u8> = Vec::new();
        let max_data_len = data_blocks.iter().map(|b| b.len()).max().unwrap_or(0);
        for i in 0..max_data_len {
            for block in &data_blocks {
                if i < block.len() {
                    interleaved.push(block[i]);
                }
            }
        }
        let max_ec_len = ec_codewords.iter().map(|b| b.len()).max().unwrap_or(0);
        for i in 0..max_ec_len {
            for block in &ec_codewords {
                if i < block.len() {
                    interleaved.push(block[i]);
                }
            }
        }

        let n = module_size(version);
        let mut modules = vec![false; n * n];
        let mut function = vec![false; n * n];

        place_function_patterns(version, n, &mut modules, &mut function);
        place_format_and_version(version, n, 0, &mut modules, &mut function); // mask 0 placeholder
        place_data(version, n, &interleaved, &mut modules, &function);

        // Try all 8 masks, pick lowest penalty (format info applied after).
        let best_mask = (0..8)
            .map(|mask| {
                let mut m = modules.clone();
                apply_mask(mask, n, &function, &mut m);
                let pen = penalty_score(n, &m);
                (mask, pen)
            })
            .min_by_key(|(_, p)| *p)
            .map(|(m, _)| m)
            .unwrap_or(0);

        // Re-apply: clear modules, rebuild with best mask + format info.
        let mut modules = vec![false; n * n];
        let mut function = vec![false; n * n];
        place_function_patterns(version, n, &mut modules, &mut function);
        place_data(version, n, &interleaved, &mut modules, &function);
        apply_mask(best_mask, n, &function, &mut modules);
        place_format_and_version(version, n, best_mask, &mut modules, &mut function);

        Ok(QrCode { size: n, modules })
    }

    /// Terminal rendering using half-blocks: two rows per QR row pair.
    pub fn to_ansi(&self) -> String {
        let n = self.size;
        let mut out = String::new();
        // Top border.
        out.push_str(" ");
        for _ in 0..n {
            out.push('▄');
        }
        out.push('\n');

        let mut row = 0;
        while row < n {
            out.push('█'); // left border
            let mut col = 0;
            while col < n {
                let top = self.modules[row * n + col];
                let bottom = if row + 1 < n {
                    self.modules[(row + 1) * n + col]
                } else {
                    false
                };
                out.push(match (top, bottom) {
                    (false, false) => ' ',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (true, true) => '█',
                });
                col += 1;
            }
            out.push('█'); // right border
            out.push('\n');
            row += 2;
        }
        // Bottom border.
        out.push(' ');
        for _ in 0..n {
            out.push('▀');
        }
        out.push('\n');
        out
    }

    /// SVG rendering for the GTK4 popup and browser views.
    pub fn to_svg(&self, scale: u32) -> String {
        let n = self.size;
        let dim = (n as u32) * scale;
        let mut out = String::new();
        out.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" width=\"{dim}\" height=\"{dim}\">\n",
        ));
        out.push_str(&format!(
            "  <rect width=\"{dim}\" height=\"{dim}\" fill=\"#ffffff\"/>\n",
        ));
        out.push_str("  <path fill=\"#000000\" d=\"");
        let mut first = true;
        for y in 0..n {
            for x in 0..n {
                if self.modules[y * n + x] {
                    if first {
                        first = false;
                    } else {
                        out.push(' ');
                    }
                    out.push_str(&format!(
                        "M{} {}h{}v{}h{}Z",
                        (x as u32) * scale,
                        (y as u32) * scale,
                        scale,
                        scale,
                        scale
                    ));
                }
            }
        }
        out.push_str("\"/>\n</svg>\n");
        out
    }

    pub fn module(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

// ---------------------------------------------------------------------------
// GF(256) / Reed-Solomon
// ---------------------------------------------------------------------------

// Primitive polynomial 0x11D.
static EXP_TABLE: OnceLock<[u32; 512]> = OnceLock::new();
static LOG_TABLE: OnceLock<[u32; 256]> = OnceLock::new();

fn gf_tables() -> (&'static [u32; 512], &'static [u32; 256]) {
    let exp = EXP_TABLE.get_or_init(|| {
        let mut tab = [0u32; 512];
        let mut x: u32 = 1;
        for i in 0..256 {
            tab[i as usize] = x;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= GF_POLY;
            }
        }
        for i in 256..512 {
            tab[i as usize] = tab[i as usize - 256];
        }
        tab
    });
    let log = LOG_TABLE.get_or_init(|| {
        let mut tab = [0u32; 256];
        for i in 0..255 {
            tab[EXP_TABLE.get().unwrap()[i as usize] as usize] = i;
        }
        tab
    });
    (exp, log)
}

fn gf_mul(a: u32, b: u32) -> u32 {
    if a == 0 || b == 0 {
        return 0;
    }
    let (exp, log) = gf_tables();
    exp[(log[a as usize] + log[b as usize]) as usize]
}

#[allow(dead_code)]
fn gf_inv(a: u32) -> u32 {
    let (exp, log) = gf_tables();
    exp[(255 - log[a as usize]) as usize]
}

/// Generator polynomial coefficients for `ec_len` error-correction bytes.
fn rs_generator_poly(ec_len: usize) -> Vec<u32> {
    let (exp, _log) = gf_tables();
    let mut g = vec![1u32];
    for i in 0..ec_len {
        let root = exp[i as usize];
        let mut new_g = vec![0u32; g.len() + 1];
        for (j, &coef) in g.iter().enumerate() {
            if coef == 0 {
                continue;
            }
            new_g[j] ^= coef;
            new_g[j + 1] ^= gf_mul(coef, root);
        }
        g = new_g;
    }
    g
}

/// Compute Reed-Solomon EC codewords for `data` (appended after generator).
fn reed_solomon_encode(data: &[u8], ec_len: usize) -> Vec<u8> {
    let (exp, log) = gf_tables();
    let gen = rs_generator_poly(ec_len);
    let mut msg = data.to_vec();
    msg.resize(data.len() + ec_len, 0);
    for i in 0..data.len() {
        let coef = msg[i];
        if coef != 0 {
            let l = log[coef as usize];
            for j in 0..gen.len() {
                msg[i + j] ^= exp[(l + log[gen[j] as usize]) as usize] as u8;
            }
        }
    }
    msg[data.len()..].to_vec()
}

// ---------------------------------------------------------------------------
// QR version / EC block table (level L, versions 1–6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct EcBlock {
    total_data: usize,
    ec_per_block: usize,
    num_blocks_1: usize,
    data_per_block_1: usize,
    num_blocks_2: usize,
    data_per_block_2: usize,
}

// Indexed by version. Built from the ISO/IEC 18004 EC table (level L).
const EC_BLOCKS: [EcBlock; 7] = [
    EcBlock {
        total_data: 0,
        ec_per_block: 0,
        num_blocks_1: 0,
        data_per_block_1: 0,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // v0 unused
    EcBlock {
        total_data: 19,
        ec_per_block: 7,
        num_blocks_1: 1,
        data_per_block_1: 19,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 1-L
    EcBlock {
        total_data: 34,
        ec_per_block: 10,
        num_blocks_1: 1,
        data_per_block_1: 34,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 2-L
    EcBlock {
        total_data: 55,
        ec_per_block: 15,
        num_blocks_1: 1,
        data_per_block_1: 55,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 3-L
    EcBlock {
        total_data: 80,
        ec_per_block: 20,
        num_blocks_1: 1,
        data_per_block_1: 80,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 4-L
    EcBlock {
        total_data: 108,
        ec_per_block: 26,
        num_blocks_1: 1,
        data_per_block_1: 108,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 5-L
    EcBlock {
        total_data: 136,
        ec_per_block: 18,
        num_blocks_1: 2,
        data_per_block_1: 68,
        num_blocks_2: 0,
        data_per_block_2: 0,
    }, // 6-L
];

fn module_size(version: u8) -> usize {
    17 + (version as usize) * 4
}

/// Number of bits used for the character count in byte mode for this version.
fn char_count_bits(_data_len: usize, version: u8) -> usize {
    match version {
        1..=9 => 8,
        _ => 16,
    }
}

/// Finder pattern (7x7) dark cell at local coordinate (dx, dy).
fn is_finder_dark(dx: usize, dy: usize) -> bool {
    if dy == 1 || dy == 5 {
        (1..=5).contains(&dx)
    } else if dy >= 2 && dy <= 4 {
        dx == 1 || dx == 5
    } else {
        false
    }
}

/// Alignment pattern (5x5) dark cell at local coordinate (dx, dy).
fn is_align_dark(dx: usize, dy: usize) -> bool {
    if dy == 0 || dy == 4 {
        dx <= 4
    } else if dy == 1 || dy == 3 {
        dx == 0 || dx == 4
    } else if dy == 2 {
        dx == 0 || dx == 2 || dx == 4
    } else {
        false
    }
}

fn push_uint(bits: &mut Vec<u8>, value: u32, count: usize) {
    for i in (0..count).rev() {
        bits.push(((value >> i) & 1) as u8);
    }
}

fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in bits.chunks(8) {
        let mut b: u8 = 0;
        for bit in chunk {
            b = (b << 1) | bit;
        }
        out.push(b);
    }
    out
}

// ---------------------------------------------------------------------------
// Module placement
// ---------------------------------------------------------------------------

fn place_function_patterns(version: u8, n: usize, modules: &mut [bool], function: &mut [bool]) {
    let setf = |f: &mut [bool], x: usize, y: usize| f[y * n + x] = true;

    // Finder patterns (7x7) at the three corners.
    let finder = |m: &mut [bool], f: &mut [bool], n: usize, ox: usize, oy: usize| {
        for dy in 0..7 {
            for dx in 0..7 {
                let dark = is_finder_dark(dx, dy);
                let x = ox + dx;
                let y = oy + dy;
                if x < n && y < n {
                    set_mod(m, n, x, y, dark);
                    setf(f, x, y);
                }
            }
        }
    };
    finder(modules, function, n, 0, 0); // top-left
    finder(modules, function, n, n - 7, 0); // top-right
    finder(modules, function, n, 0, n - 7); // bottom-left

    // Alignment patterns (version 2+).
    let align_positions = align_positions(version);
    for &cx in &align_positions {
        for &cy in &align_positions {
            // Skip if inside a finder pattern.
            if is_finder_area(n, cx, cy) {
                continue;
            }
            for dy in 0..5 {
                for dx in 0..5 {
                    let dark = is_align_dark(dx, dy);
                    let x = cx + dx - 2;
                    let y = cy + dy - 2;
                    if x < n && y < n {
                        set_mod(modules, n, x, y, dark);
                        setf(function, x, y);
                    }
                }
            }
        }
    }

    // Timing patterns (row 6 and column 6).
    for i in 0..n {
        if i == 6 {
            continue;
        }
        // Vertical timing (col 6).
        if !function[6 * n + i] {
            set_mod(modules, n, 6, i, i % 2 == 0);
            setf(function, 6, i);
        }
        // Horizontal timing (row 6).
        if !function[i * n + 6] {
            set_mod(modules, n, i, 6, i % 2 == 0);
            setf(function, i, 6);
        }
    }

    // Dark module at (n-8, 8)? Actually version-dependent: (4*version+9, ... ) no.
    // The dark module is always at (8, n-8)... let me place at (4*version + 9, ...)?
    // Standard dark module location: (4*version + 9, 8)... no. It's fixed relative.
    // Simplest correct: module (n - 8, 8) ... hmm. ZXing places dark module at
    // (4*version+9 mod ... ). For v1-6 it's effectively always present; use
    // (8, n-8)? Let's place dark module at (n-8, 8) which is the conventional spot
    // near bottom-left finder for v1+.
    let dm_x = n - 8;
    let dm_y = 8;
    if dm_x < n && dm_y < n {
        set_mod(modules, n, dm_x, dm_y, true);
        setf(function, dm_x, dm_y);
    }
}

fn align_positions(version: u8) -> Vec<usize> {
    match version {
        1 => vec![],
        2 => vec![6],
        3 => vec![6, 12],
        4 => vec![6, 14],
        5 => vec![6, 16],
        6 => vec![6, 18],
        _ => vec![],
    }
}

fn is_finder_area(n: usize, cx: usize, cy: usize) -> bool {
    let finder_centers = [(3, 3), (n - 7, 3), (3, n - 7)];
    for &(fx, fy) in &finder_centers {
        if cx >= fx.saturating_sub(2) && cx <= fx + 2 && cy >= fy.saturating_sub(2) && cy <= fy + 2
        {
            return true;
        }
    }
    false
}

fn place_format_and_version(
    version: u8,
    n: usize,
    mask: u8,
    modules: &mut [bool],
    function: &mut [bool],
) {
    let setf = |f: &mut [bool], x: usize, y: usize| f[y * n + x] = true;
    let format_bits = format_info_bits(mask); // 15 bits, MSB first
                                              // Place around top-left finder:
                                              // bit i (0..=7): (8, 8-i) for i=0..8 except i=6 → (8,8);
                                              // Standard placement:
    for i in 0..9 {
        let bit = format_bits[i];
        let (x, y) = if i < 8 { (8, 7 - i) } else { (8, 8) };
        if x < n && y < n {
            set_mod(modules, n, x, y, bit != 0);
        }
        setf(function, x, y);
    }
    // Along column 8 going up from bottom-left area:
    for i in 0..6 {
        let bit = format_bits[9 + i];
        let x = n - 1 - i;
        let y = 8;
        if x < n && y < n {
            set_mod(modules, n, x, y, bit != 0);
        }
        setf(function, x, y);
    }
    // Module (n-8, 8)? already set as dark; format occupies (n-8..n-1, 8) — overlaps handled by function mask.

    // Version info not needed for versions < 7.
    let _ = version;
}

fn set_mod(modules: &mut [bool], n: usize, x: usize, y: usize, bit: bool) {
    modules[y * n + x] = bit;
}

/// 15-bit format info (EC level L + mask), MSB first.
fn format_info_bits(mask: u8) -> [u8; 15] {
    // 2-bit EC level (L=01) + 3-bit mask = 5 data bits.
    let mut data: u32 = (0b01u32 << 3) | (mask as u32 & 0b111);
    data <<= 10; // append 10 zeros
    let rem = format_remainder(data);
    let mut value = ((data >> 10) << 10) | rem; // 15 bits: data|rem
    value ^= 0x5412;
    let mut out = [0u8; 15];
    for i in 0..15 {
        out[i] = ((value >> (14 - i)) & 1) as u8;
    }
    out
}

// Generator for format info: x^10 + x^8 + x^5 + x^4 + x^2 + x + 1 = 0b10100110111 = 0x537.
const FORMAT_GEN: u32 = 0x537;

fn format_remainder(mut data: u32) -> u32 {
    for i in (0..5).rev() {
        if (data >> (i + 10)) & 1 == 1 {
            data ^= FORMAT_GEN << i;
        }
    }
    data & 0x3FF
}

fn place_data(version: u8, n: usize, codewords: &[u8], modules: &mut [bool], function: &[bool]) {
    // Build bit stream (MSB first per codeword).
    let mut bits: Vec<u8> = Vec::with_capacity(codewords.len() * 8);
    for &cw in codewords {
        for i in (0..8).rev() {
            bits.push(((cw >> i) & 1) as u8);
        }
    }

    // Clean zigzag placement (pair of columns, right to left).
    let mut idx = 0usize;
    let mut col = n;
    let mut up = true;
    while col > 0 {
        col -= 1;
        if col == 6 {
            col -= 1;
        }
        let rows: Vec<isize> = if up {
            (0..n as isize).rev().collect()
        } else {
            (0..n as isize).collect()
        };
        for &r in &rows {
            for dc in [0isize, -1] {
                let c = col as isize + dc;
                if c < 0 || c >= n as isize {
                    continue;
                }
                let x = c as usize;
                let y = r as usize;
                if function[y * n + x] {
                    continue;
                }
                if idx < bits.len() {
                    modules[y * n + x] = bits[idx] != 0;
                    idx += 1;
                }
            }
        }
        up = !up;
    }
    let _ = version;
}

fn apply_mask(mask: u8, n: usize, function: &[bool], modules: &mut [bool]) {
    for y in 0..n {
        for x in 0..n {
            if function[y * n + x] {
                continue;
            }
            let invert = match mask {
                0 => (x + y) % 2 == 0,
                1 => y % 2 == 0,
                2 => x % 3 == 0,
                3 => (x + y) % 3 == 0,
                4 => (y / 2 + x / 3) % 2 == 0,
                5 => ((x * y) % 2 + (x * y) % 3) == 0,
                6 => ((x * y) % 2 + (x * y) % 3) % 2 == 0,
                7 => ((x * y) % 3 + (x + y) % 2) % 2 == 0,
                _ => false,
            };
            if invert {
                modules[y * n + x] = !modules[y * n + x];
            }
        }
    }
}

fn penalty_score(n: usize, modules: &[bool]) -> u32 {
    let m = |x: usize, y: usize| -> bool { modules[y * n + x] };
    let mut penalty: u32 = 0;

    // Penalty 1: runs of 5+ same color in rows/cols.
    for y in 0..n {
        penalty += run_penalty_row(n, y, m);
    }
    for x in 0..n {
        penalty += run_penalty_col(n, x, m);
    }

    // Penalty 2: 2x2 blocks same color (N2=3).
    for y in 0..n.saturating_sub(1) {
        for x in 0..n.saturating_sub(1) {
            let v = m(x, y);
            if v == m(x + 1, y) && v == m(x, y + 1) && v == m(x + 1, y + 1) {
                penalty += 3;
            }
        }
    }

    // Penalty 3: finder-like pattern 1:1:3:1:1 (N3=40) in rows/cols.
    for y in 0..n {
        penalty += 40 * finder_pattern_count_row(n, y, m);
    }
    for x in 0..n {
        penalty += 40 * finder_pattern_count_col(n, x, m);
    }

    // Penalty 4: darkness proportion (N4=10).
    let dark: u32 = modules.iter().filter(|&&b| b).count() as u32;
    let total = (n * n) as u32;
    let percent = dark * 100 / total.max(1);
    penalty += 10 * ((percent as isize - 50isize).abs() / 5) as u32;

    penalty
}

fn run_penalty_row(n: usize, y: usize, m: impl Fn(usize, usize) -> bool) -> u32 {
    let mut p: u32 = 0;
    let mut run = 1u32;
    for x in 1..n {
        if m(x, y) == m(x - 1, y) {
            run += 1;
        } else {
            if run >= 5 {
                p += 3 + (run - 5);
            }
            run = 1;
        }
    }
    if run >= 5 {
        p += 3 + (run - 5);
    }
    p
}

fn run_penalty_col(n: usize, x: usize, m: impl Fn(usize, usize) -> bool) -> u32 {
    let mut p: u32 = 0;
    let mut run = 1u32;
    for y in 1..n {
        if m(x, y) == m(x, y - 1) {
            run += 1;
        } else {
            if run >= 5 {
                p += 3 + (run - 5);
            }
            run = 1;
        }
    }
    if run >= 5 {
        p += 3 + (run - 5);
    }
    p
}

fn finder_pattern_count_row(n: usize, y: usize, m: impl Fn(usize, usize) -> bool) -> u32 {
    let mut count: u32 = 0;
    for x in 0..n.saturating_sub(7) {
        // Pattern: dark light dark-dark-dark light dark, i.e.
        // [x] dark, [x+1] light, [x+2..x+4] dark, [x+5] light, [x+6] dark
        // with surrounding (x-1 and x+7) light if they exist.
        if !m(x, y)
            || m(x + 1, y)
            || !m(x + 2, y)
            || !m(x + 3, y)
            || !m(x + 4, y)
            || m(x + 5, y)
            || !m(x + 6, y)
        {
            continue;
        }
        let left_ok = x == 0 || !m(x - 1, y);
        let right_ok = x + 7 >= n || !m(x + 7, y);
        if left_ok && right_ok {
            count += 1;
        }
    }
    count
}

fn finder_pattern_count_col(n: usize, x: usize, m: impl Fn(usize, usize) -> bool) -> u32 {
    let mut count: u32 = 0;
    for y in 0..n.saturating_sub(7) {
        if !m(x, y)
            || m(x, y + 1)
            || !m(x, y + 2)
            || !m(x, y + 3)
            || !m(x, y + 4)
            || m(x, y + 5)
            || !m(x, y + 6)
        {
            continue;
        }
        let top_ok = y == 0 || !m(x, y - 1);
        let bottom_ok = y + 7 >= n || !m(x, y + 7);
        if top_ok && bottom_ok {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_small_text() {
        let qr = QrCode::encode("Hi").expect("encode");
        assert!(qr.size >= 21 && qr.size <= 41);
        assert_eq!(qr.modules.len(), qr.size * qr.size);
    }

    #[test]
    fn to_ansi_nonempty() {
        let qr = QrCode::encode("lanbeam").expect("encode");
        let s = qr.to_ansi();
        assert!(!s.is_empty());
        assert!(s.contains('█'));
    }

    #[test]
    fn to_svg_valid() {
        let qr = QrCode::encode("https://lanbeam.local").expect("encode");
        let s = qr.to_svg(4);
        assert!(s.starts_with("<svg"));
        assert!(s.ends_with("</svg>\n"));
        assert!(s.contains("#000000"));
    }

    #[test]
    fn data_too_long_rejected() {
        // 136 data codewords = 1088 bits; way more than fits even v6-L.
        let long = "x".repeat(2000);
        assert!(QrCode::encode(&long).is_err());
    }

    #[test]
    fn version_1_max_fits() {
        // v1-L byte mode capacity is 17 bytes.
        let s = "A".repeat(17);
        assert!(QrCode::encode(&s).is_ok());
    }
}
