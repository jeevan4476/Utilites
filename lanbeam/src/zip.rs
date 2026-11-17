//! On-the-fly streaming ZIP archive packer.
//!
//! Writes standard ZIP local file headers + data descriptors and a central
//! directory at the end, so multiple files / a directory can be downloaded as
//! a single `.zip` without external archive binaries.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;

pub const ZIP_LOCAL_HEADER_SIG: u32 = 0x04034b50;
pub const ZIP_CENTRAL_DIR_SIG: u32 = 0x02014b50;
pub const ZIP_END_CENTRAL_DIR_SIG: u32 = 0x06054b50;
pub const ZIP_DATA_DESCRIPTOR_SIG: u32 = 0x08074b50;

/// Method: stored (no compression).
const METHOD_STORED: u16 = 0;

/// Streaming read buffer size (64 KiB).
const ZIP_CHUNK_SIZE: usize = 64 * 1024;

/// A single in-memory entry to be written into the ZIP stream.
pub struct ZipEntry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

/// A file-backed entry streamed in chunks without loading the file into RAM.
pub struct ZipFileEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Write sink that counts bytes and updates CRC32 incrementally.
struct ZipDataSink<'a, W: Write> {
    write: &'a mut W,
    crc: u32,
    written: u64,
}

impl<W: Write> Write for ZipDataSink<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write.write_all(buf)?;
        self.crc = crc32_update(self.crc, buf);
        self.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write.flush()
    }
}

/// Stream a ZIP archive containing `entries` into `write`.
///
/// Uses data descriptors (bit 3 of the general-purpose flag) so each entry's
/// compressed/uncompressed sizes and CRC32 are written *after* the data — this
/// lets us stream entries of unknown size without seeking.
pub fn write_zip<W: Write>(write: W, entries: &[ZipEntry<'_>]) -> io::Result<()> {
    write_zip_stream(
        write,
        entries.iter().map(|entry| {
            let data = entry.data;
            (entry.name, move |sink: &mut dyn Write| sink.write_all(data))
        }),
    )
}

/// Stream a ZIP archive of disk files in 64 KiB chunks into `write`.
pub fn write_zip_files<W: Write>(write: W, entries: &[ZipFileEntry]) -> io::Result<()> {
    write_zip_stream(
        write,
        entries.iter().map(|entry| {
            let path = entry.path.clone();
            (entry.name.as_str(), move |sink: &mut dyn Write| {
                let mut file = File::open(&path)?;
                let mut buf = [0u8; ZIP_CHUNK_SIZE];
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    sink.write_all(&buf[..n])?;
                }
                Ok(())
            })
        }),
    )
}

fn write_zip_stream<'a, W, I, F>(mut write: W, entries: I) -> io::Result<()>
where
    W: Write,
    I: IntoIterator<Item = (&'a str, F)>,
    F: FnOnce(&mut dyn Write) -> io::Result<()>,
{
    let mut central_dir: Vec<u8> = Vec::new();
    let mut offset: u64 = 0;
    let mut entry_count: usize = 0;

    for (name, stream_fn) in entries {
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ZIP entry name exceeds 64 KiB")
        })?;
        let entry_offset = u32::try_from(offset).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ZIP archive exceeds 4 GiB ZIP32 limit")
        })?;

        // Local file header (data-descriptor bit set: CRC and sizes are 0 here).
        let flag: u16 = 0x0008;
        let local_header = build_local_header(name_len, flag, METHOD_STORED, 0);
        write.write_all(&local_header)?;
        write.write_all(name_bytes)?;

        let mut sink = ZipDataSink {
            write: &mut write,
            crc: 0xFFFF_FFFF,
            written: 0,
        };
        stream_fn(&mut sink)?;
        let crc = !sink.crc;
        let written = sink.written;
        let size32 = u32::try_from(written).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ZIP entry exceeds 4 GiB ZIP32 limit")
        })?;

        // Data descriptor.
        let mut desc = Vec::with_capacity(16);
        desc.write_all(&ZIP_DATA_DESCRIPTOR_SIG.to_le_bytes())?;
        desc.write_all(&crc.to_le_bytes())?;
        desc.write_all(&size32.to_le_bytes())?;
        desc.write_all(&size32.to_le_bytes())?;
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
        cde.write_all(&size32.to_le_bytes())?; // compressed size
        cde.write_all(&size32.to_le_bytes())?; // uncompressed size
        cde.write_all(&name_len.to_le_bytes())?; // name length
        cde.write_all(&0u16.to_le_bytes())?; // extra field length
        cde.write_all(&0u16.to_le_bytes())?; // comment length
        cde.write_all(&0u16.to_le_bytes())?; // disk start
        cde.write_all(&0u16.to_le_bytes())?; // internal attr
        cde.write_all(&0u32.to_le_bytes())?; // external attr
        cde.write_all(&entry_offset.to_le_bytes())?; // local header offset
        cde.write_all(name_bytes)?;
        central_dir.extend_from_slice(&cde);

        offset += local_header.len() as u64 + name_bytes.len() as u64 + written + desc.len() as u64;
        entry_count += 1;
    }

    // End of central directory.
    let cd_start = u32::try_from(offset).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "ZIP archive exceeds 4 GiB ZIP32 limit")
    })?;
    let count16 = u16::try_from(entry_count).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "ZIP archive exceeds 65535 entries")
    })?;
    write.write_all(&central_dir)?;
    let mut eocd = Vec::with_capacity(22);
    eocd.write_all(&ZIP_END_CENTRAL_DIR_SIG.to_le_bytes())?;
    eocd.write_all(&0u16.to_le_bytes())?; // disk number
    eocd.write_all(&0u16.to_le_bytes())?; // cd disk
    eocd.write_all(&count16.to_le_bytes())?; // entries on this disk
    eocd.write_all(&count16.to_le_bytes())?; // total entries
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

/// Update an in-progress CRC32 state (initialized with `0xFFFF_FFFF`).
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
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
    crc
}

/// CRC32 (ISO 3309 / same polynomial as ZIP).
#[cfg(test)]
fn crc32(data: &[u8]) -> u32 {
    !crc32_update(0xFFFF_FFFF, data)
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
    fn write_zip_multi_entry_sets_exact_central_dir_offsets() {
        let entries = [
            ZipEntry {
                name: "a.txt",
                data: b"alpha",
            },
            ZipEntry {
                name: "b.txt",
                data: b"bravo",
            },
        ];
        let mut buf = Vec::new();
        write_zip(&mut buf, &entries).expect("write zip");

        let eocd_start = buf.len() - 22;
        let cd_offset =
            u32::from_le_bytes(buf[eocd_start + 16..eocd_start + 20].try_into().unwrap()) as usize;
        assert_eq!(&buf[cd_offset..cd_offset + 4], &ZIP_CENTRAL_DIR_SIG.to_le_bytes());

        let second_local_offset =
            30 + "a.txt".len() + "alpha".len() + 16;
        assert_eq!(
            &buf[second_local_offset..second_local_offset + 4],
            &ZIP_LOCAL_HEADER_SIG.to_le_bytes()
        );
    }

    #[test]
    fn write_zip_files_streams_large_file() {
        let dir = std::env::temp_dir().join(format!(
            "lanbeam-zip-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("big.bin");
        let payload = vec![0x5Au8; 150_000];
        std::fs::write(&file_path, &payload).unwrap();

        let mut buf = Vec::new();
        write_zip_files(
            &mut buf,
            &[ZipFileEntry {
                name: "big.bin".to_string(),
                path: file_path.clone(),
            }],
        )
        .expect("stream zip file");

        let _ = std::fs::remove_dir_all(&dir);
        assert!(buf.starts_with(&ZIP_LOCAL_HEADER_SIG.to_le_bytes()));
        assert!(buf.len() > payload.len());
    }

    #[test]
    fn crc32_known_value() {
        // CRC32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}