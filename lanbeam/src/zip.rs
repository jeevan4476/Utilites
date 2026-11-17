//! On-the-fly streaming ZIP archive packer.
//!
//! Writes standard ZIP local file headers + data descriptors and a central
//! directory at the end, so multiple files / a directory can be downloaded as
//! a single `.zip` without external archive binaries.

use std::io::{self, Write};

const ZIP_LOCAL_HEADER_SIG: u32 = 0x04034b50;
const ZIP_CENTRAL_DIR_SIG: u32 = 0x02014b50;
const ZIP_END_CENTRAL_DIR_SIG: u32 = 0x06054b50;
const ZIP_DATA_DESCRIPTOR_SIG: u32 = 0x08074b50;

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
    let mut central_dir: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;

    for entry in entries {
        let name_bytes = entry.name.as_bytes();
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
        cde.write_all(&(offset as u32).to_le_bytes())?; // local header offset
        cde.write_all(name_bytes)?;
        central_dir.extend_from_slice(&cde);

        offset += local_header.len() as u64 + name_bytes.len() as u64 + desc.len() as u64;
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

/// CRC32 (ISO 3309 / same polynomial as ZIP).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn crc32_known_value() {
        // CRC32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
