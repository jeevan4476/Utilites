//! On-the-fly streaming ZIP archive packer.
//!
//! Writes standard ZIP local file headers + data descriptors and a central
//! directory at the end, so multiple files / a directory can be downloaded as
//! a single `.zip` without external archive binaries.

use std::io::{self, Write};
use std::path::PathBuf;

pub const ZIP_LOCAL_HEADER_SIG: u32 = 0x04034b50;
pub const ZIP_CENTRAL_DIR_SIG: u32 = 0x02014b50;
pub const ZIP_END_CENTRAL_DIR_SIG: u32 = 0x06054b50;
pub const ZIP_DATA_DESCRIPTOR_SIG: u32 = 0x08074b50;

/// Method: stored (no compression).
const METHOD_STORED: u16 = 0;

/// A single entry to be written into the ZIP stream.
pub struct ZipEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// Stream a ZIP archive containing `entries` into `write`.
///
/// Uses data descriptors (bit 3 of the general-purpose flag) so each entry's
/// compressed/uncompressed sizes and CRC32 are written *after* the data — this
/// lets us stream entries of unknown size without seeking.
pub fn write_zip<W: Write>(mut write: W, entries: &[ZipEntry<'_>]) -> io::Result<()> {
    if entries.len() > u16::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many entries",
        ));
    }
    for entry in entries {
        if entry.data.len() > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZIP entry data exceeds u32 size limit",
            ));
        }
        if entry.name.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "filename too long",
            ));
        }
    }
    let mut central_dir: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;

    for entry in entries {
        let name_bytes = entry.name.as_bytes();

        // Bounds check: name length must fit in u16
        if name_bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("filename too long: {} > {}", name_bytes.len(), u16::MAX),
            ));
        }

        let crc = crc32(entry.data);

        // Local file header (data-descriptor bit set).
        let flag: u16 = 0x0008;
        let local_header = build_local_header(name_bytes.len() as u16, flag, METHOD_STORED, crc);
        write.write_all(&local_header)?;
        write.write_all(name_bytes)?;
        write.write_all(entry.data)?;

        // Data descriptor.
        let mut desc = Vec::with_capacity(16);
        desc.write_all(&ZIP_DATA_DESCRIPTOR_SIG.to_le_bytes())?;
        desc.write_all(&crc.to_le_bytes())?;
        // Data descriptor sizes (0xFFFFFFFF placeholder not needed; we write real values).
        desc.write_all(&(entry.data.len() as u32).to_le_bytes())?;
        desc.write_all(&(entry.data.len() as u32).to_le_bytes())?;
        write.write_all(&desc)?;

        // Central directory entry.
        let mut cde = Vec::with_capacity(46 + name_bytes.len());
        cde.write_all(&ZIP_CENTRAL_DIR_SIG.to_le_bytes())?;
        cde.write_all(&20u16.to_le_bytes())?; // version made by
        cde.write_all(&20u16.to_le_bytes())?; // version needed
        cde.write_all(&flag.to_le_bytes())?;
        cde.write_all(&METHOD_STORED.to_le_bytes())?;
        cde.write_all(&0u16.to_le_bytes())?; // mod time
        cde.write_all(&0u16.to_le_bytes())?; // mod date
        cde.write_all(&crc.to_le_bytes())?;
        cde.write_all(&(entry.data.len() as u32).to_le_bytes())?; // compressed size
        cde.write_all(&(entry.data.len() as u32).to_le_bytes())?; // uncompressed size
        cde.write_all(&(name_bytes.len() as u16).to_le_bytes())?; // name length
        cde.write_all(&0u16.to_le_bytes())?; // extra field length
        cde.write_all(&0u16.to_le_bytes())?; // comment length
        cde.write_all(&0u16.to_le_bytes())?; // disk start
        cde.write_all(&0u16.to_le_bytes())?; // internal attr
        cde.write_all(&0u32.to_le_bytes())?; // external attr
                                             // Bounds check: offset must fit in u32
        if offset > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("local header offset too large: {} > {}", offset, u32::MAX),
            ));
        }
        cde.write_all(&(offset as u32).to_le_bytes())?; // local header offset
        cde.write_all(name_bytes)?;
        central_dir.extend_from_slice(&cde);

        offset += local_header.len() as u64
            + name_bytes.len() as u64
            + entry.data.len() as u64
            + desc.len() as u64;
    }

    // Bounds check: total entries must fit in u16
    if entries.len() > u16::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("too many entries: {} > {}", entries.len(), u16::MAX),
        ));
    }

    // Bounds check: central directory size must fit in u32
    if central_dir.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "central directory too large: {} > {}",
                central_dir.len(),
                u32::MAX
            ),
        ));
    }

    // Bounds check: cd_start offset must fit in u32
    if offset > u32::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "central directory start offset too large: {} > {}",
                offset,
                u32::MAX
            ),
        ));
    }

    // End of central directory.
    let cd_start = offset as u32;
    write.write_all(&central_dir)?;
    let mut eocd = Vec::with_capacity(22);
    eocd.write_all(&ZIP_END_CENTRAL_DIR_SIG.to_le_bytes())?;
    eocd.write_all(&0u16.to_le_bytes())?; // disk number
    eocd.write_all(&0u16.to_le_bytes())?; // cd disk
    eocd.write_all(&(entries.len() as u16).to_le_bytes())?; // entries on this disk
    eocd.write_all(&(entries.len() as u16).to_le_bytes())?; // total entries
    eocd.write_all(&(central_dir.len() as u32).to_le_bytes())?; // cd size
    eocd.write_all(&cd_start.to_le_bytes())?; // cd offset
    eocd.write_all(&0u16.to_le_bytes())?; // comment length
    write.write_all(&eocd)?;

    Ok(())
}

/// Build a ZIP local file header (30 bytes + name length).
fn build_local_header(name_len: u16, flag: u16, method: u16, crc: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(30);
    h.write_all(&ZIP_LOCAL_HEADER_SIG.to_le_bytes()).unwrap();
    h.write_all(&20u16.to_le_bytes()).unwrap(); // version needed
    h.write_all(&flag.to_le_bytes()).unwrap();
    h.write_all(&method.to_le_bytes()).unwrap();
    h.write_all(&0u16.to_le_bytes()).unwrap(); // mod time
    h.write_all(&0u16.to_le_bytes()).unwrap(); // mod date
    h.write_all(&crc.to_le_bytes()).unwrap();
    h.write_all(&0u32.to_le_bytes()).unwrap(); // compressed size (descriptor)
    h.write_all(&0u32.to_le_bytes()).unwrap(); // uncompressed size (descriptor)
    h.write_all(&name_len.to_le_bytes()).unwrap();
    h.write_all(&0u16.to_le_bytes()).unwrap(); // extra field length
    h
}

/// Compile-time CRC32 slice-by-8 lookup tables.
const fn build_crc32_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
            bit += 1;
        }
        tables[0][i] = c;
        i += 1;
    }
    let mut table = 1;
    while table < 8 {
        i = 0;
        while i < 256 {
            let prior = tables[table - 1][i];
            tables[table][i] = tables[0][(prior & 0xff) as usize] ^ (prior >> 8);
            i += 1;
        }
        table += 1;
    }
    tables
}

const CRC32_TABLES: [[u32; 256]; 8] = build_crc32_tables();

/// CRC32 (ISO 3309 / same polynomial as ZIP).
pub fn crc32(data: &[u8]) -> u32 {
    let mut hasher = Crc32::new();
    hasher.update(data);
    hasher.finalize()
}

/// Streaming CRC32 calculator.
pub struct Crc32 {
    crc: u32,
}

impl Crc32 {
    pub fn new() -> Self {
        Self { crc: 0xFFFF_FFFF }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut remaining = data;
        while remaining.len() >= 8 {
            let first = self.crc ^ u32::from_le_bytes(remaining[..4].try_into().unwrap());
            self.crc = CRC32_TABLES[7][(first & 0xff) as usize]
                ^ CRC32_TABLES[6][((first >> 8) & 0xff) as usize]
                ^ CRC32_TABLES[5][((first >> 16) & 0xff) as usize]
                ^ CRC32_TABLES[4][(first >> 24) as usize]
                ^ CRC32_TABLES[3][remaining[4] as usize]
                ^ CRC32_TABLES[2][remaining[5] as usize]
                ^ CRC32_TABLES[1][remaining[6] as usize]
                ^ CRC32_TABLES[0][remaining[7] as usize];
            remaining = &remaining[8..];
        }
        for &byte in remaining {
            let idx = ((self.crc as u8) ^ byte) as usize;
            self.crc = (self.crc >> 8) ^ CRC32_TABLES[0][idx];
        }
    }

    pub fn finalize(self) -> u32 {
        !self.crc
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Stream a ZIP archive from file paths in a single pass.
///
/// For each entry `(name, path)`:
/// - Writes local file header with `crc=0, size=0` and general-purpose bit `0x0008`
/// - Streams file contents in 64 KiB chunks, computing CRC32 and byte count
/// - Writes 16-byte Data Descriptor with actual CRC and sizes
/// - Records Central Directory entry
///
/// Returns the count of successfully written entries (skips entries that fail to open
/// before writing headers, keeping EOCD count exact).
pub fn stream_zip_entries<W: Write>(
    mut write: W,
    entries: &[(String, PathBuf)],
) -> io::Result<u32> {
    const CHUNK_SIZE: usize = 65536;
    let mut central_dir: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;
    let mut written_count: u32 = 0;

    // Single reusable buffer for all files
    let mut buf = vec![0u8; CHUNK_SIZE];

    for (name, path) in entries {
        let name_bytes = name.as_bytes();

        // Bounds check: name length must fit in u16
        if name_bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("filename too long: {} > {}", name_bytes.len(), u16::MAX),
            ));
        }

        // Open file first; skip if it fails (before writing any headers)
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue, // Skip this entry, don't increment written_count
        };

        let file_size = file.metadata()?.len();
        if file_size > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZIP entry data exceeds u32 size limit",
            ));
        }
        if offset > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZIP local header offset exceeds u32 size limit",
            ));
        }
        let projected_end = offset
            .checked_add(30 + name_bytes.len() as u64 + file_size + 16)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "ZIP archive offset overflow")
            })?;
        if projected_end > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZIP central directory offset exceeds u32 size limit",
            ));
        }

        // Local file header (data-descriptor bit set, crc=0, sizes=0)
        let flag: u16 = 0x0008;
        let local_header = build_local_header(name_bytes.len() as u16, flag, METHOD_STORED, 0);
        write.write_all(&local_header)?;
        write.write_all(name_bytes)?;

        // Stream file data in chunks, computing CRC32 and total size
        let mut crc = Crc32::new();
        let mut total_size: u64 = 0;

        loop {
            let n = std::io::Read::read(&mut file, &mut buf)?;
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            crc.update(chunk);
            total_size += n as u64;
            write.write_all(chunk)?;
        }

        // Bounds check: total_size must fit in u32
        if total_size > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file too large for ZIP: {} > {}", total_size, u32::MAX),
            ));
        }

        let final_crc = crc.finalize();

        // Data descriptor (16 bytes: signature + crc + compressed_size + uncompressed_size)
        let mut desc = Vec::with_capacity(16);
        desc.write_all(&ZIP_DATA_DESCRIPTOR_SIG.to_le_bytes())?;
        desc.write_all(&final_crc.to_le_bytes())?;
        desc.write_all(&(total_size as u32).to_le_bytes())?;
        desc.write_all(&(total_size as u32).to_le_bytes())?;
        write.write_all(&desc)?;

        // Central directory entry
        let mut cde = Vec::with_capacity(46 + name_bytes.len());
        cde.write_all(&ZIP_CENTRAL_DIR_SIG.to_le_bytes())?;
        cde.write_all(&20u16.to_le_bytes())?; // version made by
        cde.write_all(&20u16.to_le_bytes())?; // version needed
        cde.write_all(&flag.to_le_bytes())?;
        cde.write_all(&METHOD_STORED.to_le_bytes())?;
        cde.write_all(&0u16.to_le_bytes())?; // mod time
        cde.write_all(&0u16.to_le_bytes())?; // mod date
        cde.write_all(&final_crc.to_le_bytes())?;
        cde.write_all(&(total_size as u32).to_le_bytes())?; // compressed size
        cde.write_all(&(total_size as u32).to_le_bytes())?; // uncompressed size
        cde.write_all(&(name_bytes.len() as u16).to_le_bytes())?; // name length
        cde.write_all(&0u16.to_le_bytes())?; // extra field length
        cde.write_all(&0u16.to_le_bytes())?; // comment length
        cde.write_all(&0u16.to_le_bytes())?; // disk start
        cde.write_all(&0u16.to_le_bytes())?; // internal attr
        cde.write_all(&0u32.to_le_bytes())?; // external attr
                                             // Bounds check: offset must fit in u32
        if offset > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("local header offset too large: {} > {}", offset, u32::MAX),
            ));
        }
        cde.write_all(&(offset as u32).to_le_bytes())?; // local header offset
        cde.write_all(name_bytes)?;
        central_dir.extend_from_slice(&cde);

        offset +=
            local_header.len() as u64 + name_bytes.len() as u64 + total_size + desc.len() as u64;
        written_count += 1;
    }

    // Bounds check: written_count must fit in u16
    if written_count > u16::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("too many written entries: {} > {}", written_count, u16::MAX),
        ));
    }

    // Bounds check: central directory size must fit in u32
    if central_dir.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "central directory too large: {} > {}",
                central_dir.len(),
                u32::MAX
            ),
        ));
    }

    // Bounds check: cd_start offset must fit in u32
    if offset > u32::MAX as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "central directory start offset too large: {} > {}",
                offset,
                u32::MAX
            ),
        ));
    }

    // End of central directory - use written_count, not entries.len()
    let cd_start = offset as u32;
    write.write_all(&central_dir)?;
    let mut eocd = Vec::with_capacity(22);
    eocd.write_all(&ZIP_END_CENTRAL_DIR_SIG.to_le_bytes())?;
    eocd.write_all(&0u16.to_le_bytes())?; // disk number
    eocd.write_all(&0u16.to_le_bytes())?; // cd disk
    eocd.write_all(&(written_count as u16).to_le_bytes())?; // entries on this disk
    eocd.write_all(&(written_count as u16).to_le_bytes())?; // total entries
    eocd.write_all(&(central_dir.len() as u32).to_le_bytes())?; // cd size
    eocd.write_all(&cd_start.to_le_bytes())?; // cd offset
    eocd.write_all(&0u16.to_le_bytes())?; // comment length
    write.write_all(&eocd)?;

    Ok(written_count)
}

/// Parse a ZIP local file header from bytes, returning (name_len, extra_len, header_size)
#[allow(dead_code)]
fn parse_local_header(buf: &[u8]) -> io::Result<(u16, u16, usize)> {
    if buf.len() < 30 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated local header",
        ));
    }
    let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if sig != ZIP_LOCAL_HEADER_SIG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad local header signature",
        ));
    }
    let name_len = u16::from_le_bytes([buf[26], buf[27]]);
    let extra_len = u16::from_le_bytes([buf[28], buf[29]]);
    let header_size = 30 + name_len as usize + extra_len as usize;
    Ok((name_len, extra_len, header_size))
}

/// Parse a ZIP data descriptor from bytes, returning (crc32, compressed_size, uncompressed_size)
#[allow(dead_code)]
fn parse_data_descriptor(buf: &[u8]) -> io::Result<(u32, u32, u32)> {
    if buf.len() < 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated data descriptor",
        ));
    }
    let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if sig != ZIP_DATA_DESCRIPTOR_SIG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad data descriptor signature",
        ));
    }
    let crc = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let compressed = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let uncompressed = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    Ok((crc, compressed, uncompressed))
}

/// Parse a central directory entry from bytes, returning (name_len, extra_len, comment_len, header_size, local_offset)
#[allow(dead_code)]
fn parse_central_dir_entry(buf: &[u8]) -> io::Result<(u16, u16, u16, usize, u32)> {
    if buf.len() < 46 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated central dir entry",
        ));
    }
    let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if sig != ZIP_CENTRAL_DIR_SIG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad central dir signature",
        ));
    }
    let name_len = u16::from_le_bytes([buf[28], buf[29]]);
    let extra_len = u16::from_le_bytes([buf[30], buf[31]]);
    let comment_len = u16::from_le_bytes([buf[32], buf[33]]);
    let local_offset = u32::from_le_bytes([buf[42], buf[43], buf[44], buf[45]]);
    let header_size = 46 + name_len as usize + extra_len as usize + comment_len as usize;
    Ok((name_len, extra_len, comment_len, header_size, local_offset))
}

/// Parse EOCD from bytes, returning (total_entries, cd_size, cd_offset)
#[allow(dead_code)]
fn parse_eocd(buf: &[u8]) -> io::Result<(u16, u32, u32)> {
    if buf.len() < 22 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated EOCD"));
    }
    let sig = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if sig != ZIP_END_CENTRAL_DIR_SIG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad EOCD signature",
        ));
    }
    let total_entries = u16::from_le_bytes([buf[10], buf[11]]);
    let cd_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let cd_offset = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    Ok((total_entries, cd_size, cd_offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), id))
    }

    #[test]
    fn round_trip_single_entry() {
        let data = b"hello, lanbeam zip!";
        let entries = [ZipEntry {
            name: "hello.txt",
            data,
        }];
        let mut buf = Vec::new();
        write_zip(&mut buf, &entries).expect("write zip");

        // Minimal structural sanity checks.
        assert!(buf.starts_with(&ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        let eocd_start = buf.len() - 22;
        assert!(buf[eocd_start..].starts_with(&ZIP_END_CENTRAL_DIR_SIG.to_le_bytes()));
        assert!(buf.windows(b"hello.txt".len()).any(|w| w == b"hello.txt"));
    }

    #[test]
    fn round_trip_multi_entry() {
        let entries = [
            ZipEntry {
                name: "a.txt",
                data: b"content a",
            },
            ZipEntry {
                name: "b.txt",
                data: b"content b",
            },
            ZipEntry {
                name: "sub/c.txt",
                data: b"content c",
            },
        ];
        let mut buf = Vec::new();
        write_zip(&mut buf, &entries).expect("write zip");

        // Check EOCD entry count
        let eocd_start = buf.len() - 22;
        let eocd = &buf[eocd_start..];
        let entries_on_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
        let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
        assert_eq!(entries_on_disk, 3);
        assert_eq!(total_entries, 3);

        // Check central directory offsets advance correctly
        // First local header at offset 0
        // Second local header at offset after first entry
        // Third local header at offset after second entry
        assert!(buf.windows(b"a.txt".len()).any(|w| w == b"a.txt"));
        assert!(buf.windows(b"b.txt".len()).any(|w| w == b"b.txt"));
        assert!(buf.windows(b"sub/c.txt".len()).any(|w| w == b"sub/c.txt"));
    }

    #[test]
    fn crc32_known_value() {
        // CRC32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn streaming_crc32_matches_batch() {
        let data = b"streaming crc32 test data for verification";
        let batch = crc32(data);

        let mut streaming = Crc32::new();
        streaming.update(&data[..10]);
        streaming.update(&data[10..20]);
        streaming.update(&data[20..]);
        let streamed = streaming.finalize();

        assert_eq!(batch, streamed);
    }

    #[test]
    fn crc32_slice_by8_lengths() {
        // Reference: byte-by-byte CRC32 (using standard table)
        fn crc32_ref(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &byte in data {
                let idx = ((crc as u8) ^ byte) as usize;
                crc = (crc >> 8) ^ CRC32_TABLES[0][idx];
            }
            !crc
        }

        // Test lengths: 0, 1..=7, 8, 9..=15, 64, 65537
        let test_lengths = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 64, 65537,
        ];

        for &len in &test_lengths {
            let data = vec![0xA5u8; len];
            let expected = crc32_ref(&data);
            let actual = crc32(&data);
            assert_eq!(
                actual, expected,
                "length {} mismatch: expected {:08X}, got {:08X}",
                len, expected, actual
            );
        }
        // Known test vector
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn stream_zip_entries_single() {
        // Create temp file
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"hello from stream_zip_entries").unwrap();
        drop(f);

        let entries = vec![("test.txt".to_string(), file_path)];
        let mut buf = Vec::new();
        let count = stream_zip_entries(&mut buf, &entries).unwrap();

        assert_eq!(count, 1);
        assert!(buf.starts_with(&ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        let eocd_start = buf.len() - 22;
        assert!(buf[eocd_start..].starts_with(&ZIP_END_CENTRAL_DIR_SIG.to_le_bytes()));
        assert!(buf.windows(b"test.txt".len()).any(|w| w == b"test.txt"));
        assert!(buf
            .windows(b"hello from stream_zip_entries".len())
            .any(|w| w == b"hello from stream_zip_entries"));
    }

    #[test]
    fn stream_zip_entries_multi() {
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file1 = tmp_dir.join("a.txt");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"content a").unwrap();
        drop(f);

        let file2 = tmp_dir.join("b.txt");
        let mut f = File::create(&file2).unwrap();
        f.write_all(b"content b").unwrap();
        drop(f);

        let entries = vec![("a.txt".to_string(), file1), ("b.txt".to_string(), file2)];
        let mut buf = Vec::new();
        let count = stream_zip_entries(&mut buf, &entries).unwrap();

        assert_eq!(count, 2);
        let eocd_start = buf.len() - 22;
        let eocd = &buf[eocd_start..];
        let entries_on_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
        let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
        assert_eq!(entries_on_disk, 2);
        assert_eq!(total_entries, 2);
    }

    #[test]
    fn stream_zip_entries_skips_missing() {
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file1 = tmp_dir.join("exists.txt");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"exists").unwrap();
        drop(f);

        let missing = tmp_dir.join("missing.txt");
        let entries = vec![
            ("exists.txt".to_string(), file1),
            ("missing.txt".to_string(), missing),
        ];
        let mut buf = Vec::new();
        let count = stream_zip_entries(&mut buf, &entries).unwrap();

        assert_eq!(count, 1); // Only one written
        let eocd_start = buf.len() - 22;
        let eocd = &buf[eocd_start..];
        let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
        assert_eq!(total_entries, 1); // EOCD count is exact
    }

    #[test]
    fn write_zip_central_dir_offsets() {
        // Test that central directory offsets account for entry.data.len()
        let entries = [
            ZipEntry {
                name: "a.txt",
                data: b"x",
            },
            ZipEntry {
                name: "b.txt",
                data: b"yyyy",
            },
        ];
        let mut buf = Vec::new();
        write_zip(&mut buf, &entries).unwrap();

        // Find central directory entries and check their local header offsets
        let mut offsets = Vec::new();
        let mut pos = 0;
        while pos < buf.len() {
            if pos + 4 <= buf.len()
                && u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
                    == ZIP_CENTRAL_DIR_SIG
            {
                if pos + 46 <= buf.len() {
                    let local_offset = u32::from_le_bytes([
                        buf[pos + 42],
                        buf[pos + 43],
                        buf[pos + 44],
                        buf[pos + 45],
                    ]);
                    offsets.push(local_offset);
                }
                pos += 4;
            } else {
                pos += 1;
            }
        }

        assert_eq!(offsets.len(), 2);
        // First entry at offset 0
        assert_eq!(offsets[0], 0);
        // Second entry offset = first local_header + name + data + descriptor
        // local_header = 30 bytes, name = 5 bytes ("a.txt"), data = 1 byte, descriptor = 16 bytes = 52
        assert_eq!(offsets[1], 52);
    }

    #[test]
    fn write_zip_name_too_long_error() {
        let long_name = "a".repeat(u16::MAX as usize + 1);
        let entries = [ZipEntry {
            name: &long_name,
            data: b"x",
        }];
        let mut buf = Vec::new();
        let result = write_zip(&mut buf, &entries);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("filename too long"));
    }

    #[test]
    fn write_zip_too_many_entries_error() {
        let entries: Vec<ZipEntry> = (0..=u16::MAX)
            .map(|i| ZipEntry {
                name: Box::leak(format!("file{}.txt", i).into_boxed_str()),
                data: b"x",
            })
            .collect();
        let mut buf = Vec::new();
        let result = write_zip(&mut buf, &entries);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too many entries"));
    }

    #[test]
    fn stream_zip_entries_name_too_long_error() {
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("test.txt");
        let mut f = File::create(&file_path).unwrap();
        f.write_all(b"test").unwrap();
        drop(f);

        let long_name = "a".repeat(u16::MAX as usize + 1);
        let entries = vec![(long_name, file_path)];
        let mut buf = Vec::new();
        let result = stream_zip_entries(&mut buf, &entries);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("filename too long"));
    }

    #[test]
    fn stream_zip_entries_file_too_large_error() {
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file_path = tmp_dir.join("huge.txt");
        let file = File::create(&file_path).unwrap();
        file.set_len(u32::MAX as u64 + 1).unwrap(); // sparse file; no 4 GiB allocation
        drop(file);
        let entries = vec![("huge.txt".to_string(), file_path)];
        let mut buf = Vec::new();
        let err = stream_zip_entries(&mut buf, &entries).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            buf.is_empty(),
            "oversized metadata must be rejected before writing a header"
        );
    }

    #[test]
    fn stream_zip_entries_parses_and_verifies_crc_and_payloads() {
        let tmp_dir = unique_temp_dir("lanbeam_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());

        let file1 = tmp_dir.join("a.txt");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"content a").unwrap();
        drop(f);

        let file2 = tmp_dir.join("b.txt");
        let mut f = File::create(&file2).unwrap();
        f.write_all(b"content b").unwrap();
        drop(f);

        let entries = vec![("a.txt".to_string(), file1), ("b.txt".to_string(), file2)];
        let mut buf = Vec::new();
        let count = stream_zip_entries(&mut buf, &entries).unwrap();

        assert_eq!(count, 2);

        // --- Parse and verify the streamed ZIP ---
        let mut pos = 0;

        // Parse first local header
        let (name_len1, extra_len1, header_size1) = parse_local_header(&buf[pos..]).unwrap();
        assert_eq!(name_len1, 5); // "a.txt"
        assert_eq!(extra_len1, 0);
        // Name is at pos + 30 (after 30-byte fixed header)
        let name1 = std::str::from_utf8(&buf[pos + 30..pos + 30 + name_len1 as usize]).unwrap();
        assert_eq!(name1, "a.txt");
        // Advance pos to after local header + name + extra = start of data
        pos += header_size1;

        // Read file data (until data descriptor)
        // Find data descriptor signature
        let mut data1 = Vec::new();
        while pos + 4 <= buf.len() {
            let sig = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
            if sig == ZIP_DATA_DESCRIPTOR_SIG {
                break;
            }
            data1.push(buf[pos]);
            pos += 1;
        }
        assert_eq!(data1, b"content a");

        // Parse data descriptor
        let (crc1, comp1, uncomp1) = parse_data_descriptor(&buf[pos..pos + 16]).unwrap();
        assert_eq!(comp1, 9); // "content a" = 9 bytes
        assert_eq!(uncomp1, 9);
        // Verify CRC32
        assert_eq!(crc1, crc32(b"content a"));
        pos += 16;

        // Parse second local header
        let (name_len2, extra_len2, header_size2) = parse_local_header(&buf[pos..]).unwrap();
        assert_eq!(name_len2, 5); // "b.txt"
        assert_eq!(extra_len2, 0);
        // Name is at pos + 30 (after 30-byte fixed header)
        let name2 = std::str::from_utf8(&buf[pos + 30..pos + 30 + name_len2 as usize]).unwrap();
        assert_eq!(name2, "b.txt");
        // Advance pos to after local header + name + extra = start of data
        pos += header_size2;

        // Read file data
        let mut data2 = Vec::new();
        while pos + 4 <= buf.len() {
            let sig = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
            if sig == ZIP_DATA_DESCRIPTOR_SIG {
                break;
            }
            data2.push(buf[pos]);
            pos += 1;
        }
        assert_eq!(data2, b"content b");

        // Parse data descriptor
        let (crc2, comp2, uncomp2) = parse_data_descriptor(&buf[pos..pos + 16]).unwrap();
        assert_eq!(comp2, 9);
        assert_eq!(uncomp2, 9);
        assert_eq!(crc2, crc32(b"content b"));
        pos += 16;

        // Parse central directory entries
        // Name starts at offset 46 from the start of the central directory entry
        let (cd_name_len1, cd_extra_len1, cd_comment_len1, cd_header_size1, local_offset1) =
            parse_central_dir_entry(&buf[pos..]).unwrap();
        assert_eq!(cd_name_len1, 5);
        assert_eq!(cd_extra_len1, 0);
        assert_eq!(cd_comment_len1, 0);
        assert_eq!(local_offset1, 0); // First entry at offset 0
                                      // Name is at offset 46 from start of central directory entry
        let cd_name1 =
            std::str::from_utf8(&buf[pos + 46..pos + 46 + cd_name_len1 as usize]).unwrap();
        assert_eq!(cd_name1, "a.txt");
        pos += cd_header_size1;

        let (cd_name_len2, _cd_extra_len2, _cd_comment_len2, cd_header_size2, local_offset2) =
            parse_central_dir_entry(&buf[pos..]).unwrap();
        assert_eq!(cd_name_len2, 5);
        assert_eq!(local_offset2, 60); // Second entry offset: LH(30) + name(5) + data(9) + DD(16) = 60
        let cd_name2 =
            std::str::from_utf8(&buf[pos + 46..pos + 46 + cd_name_len2 as usize]).unwrap();
        assert_eq!(cd_name2, "b.txt");
        pos += cd_header_size2;
        let _ = pos; // pos updated but not used further

        // Parse EOCD
        let eocd_start = buf.len() - 22;
        let (total_entries, cd_size, cd_offset) = parse_eocd(&buf[eocd_start..]).unwrap();
        assert_eq!(total_entries, 2);
        // Verify central directory offset (after both local file entries + data descriptors)
        // First: LH(30) + name(5) + data(9) + DD(16) = 60
        // Second: LH(30) + name(5) + data(9) + DD(16) = 60
        // Total = 120
        assert_eq!(cd_offset, 120);
        // Verify central directory size is reasonable (cd_size > 0)
        assert!(cd_size > 0);
    }

    #[test]
    fn zip_bounds_checks_offset_overflow() {
        let tmp_dir = unique_temp_dir("lanbeam_offset_test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let _guard = TempDirGuard(tmp_dir.clone());
        let huge = tmp_dir.join("huge.bin");
        let file = File::create(&huge).unwrap();
        file.set_len(u32::MAX as u64 - 40).unwrap();
        drop(file);
        let small = tmp_dir.join("small.bin");
        File::create(&small).unwrap();
        let entries = vec![
            ("huge.bin".to_string(), huge),
            ("small.bin".to_string(), small),
        ];
        let mut buf = Vec::new();
        let err = stream_zip_entries(&mut buf, &entries).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            buf.is_empty(),
            "projected archive offset must be checked before writing headers"
        );
    }

    // Temp directory cleanup guard
    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
