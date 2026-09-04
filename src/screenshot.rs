//! A dependency-free PNG writer, so the game can dump exactly what it rendered.
//!
//! This exists so the frame can be inspected outside the running window -- press
//! F2 in game, or launch with `--shot <path>` to capture one frame and exit.
//! It writes a zlib stream of stored (uncompressed) blocks, which is a perfectly
//! legal deflate stream and needs no compressor.

use std::io::Write;
use std::path::Path;

/// Write an RGBA8 image as a PNG. `pixels` is `w * h * 4` bytes, row-major.
pub fn write_rgba_png(path: &Path, w: u32, h: u32, pixels: &[u8]) -> std::io::Result<()> {
    assert_eq!(pixels.len(), (w as usize) * (h as usize) * 4);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = Vec::with_capacity(pixels.len() + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // IHDR: 8-bit RGBA, no interlace.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);

    // Raw scanlines, each prefixed by filter type 0 (None).
    let stride = (w as usize) * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * stride..(y + 1) * stride]);
    }

    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);

    let mut f = std::fs::File::create(path)?;
    f.write_all(&out)?;
    Ok(())
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream made entirely of stored deflate blocks. Bigger than a real
/// compressor would produce, but correct and about twenty lines.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    out.extend_from_slice(&[0x78, 0x01]); // CMF/FLG: deflate, 32K window, no dict

    let mut chunks = data.chunks(65535).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(part) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 1 } else { 0 });
        let n = part.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_known_check_value() {
        // The standard CRC-32 check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_matches_the_known_check_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_written_png_has_the_right_signature_and_chunks() {
        let dir = std::env::temp_dir().join("loudstone_png_test");
        let path = dir.join("t.png");
        let pixels = vec![0x40u8; 4 * 4 * 4];
        write_rgba_png(&path, 4, 4, &pixels).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR immediately follows the signature, IEND terminates the file.
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stored_blocks_split_at_the_deflate_limit() {
        // Just over one block: the stream must contain two, and only the last
        // may carry the final flag.
        let data = vec![7u8; 65535 + 10];
        let z = zlib_stored(&data);
        assert_eq!(&z[..2], &[0x78, 0x01]);
        assert_eq!(z[2], 0, "first block must not be marked final");
        let second = 2 + 5 + 65535;
        assert_eq!(z[second], 1, "second block must be marked final");
    }
}
