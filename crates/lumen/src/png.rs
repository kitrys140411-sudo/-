//! Minimal PNG encoder for screenshots (RGBA8, non-interlaced).

use std::io::Write;

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

fn crc32(table: &[u32; 256], parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for p in parts {
        for &b in *p {
            c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xffff_ffff
}

fn chunk(out: &mut Vec<u8>, table: &[u32; 256], kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(table, &[kind, data]).to_be_bytes());
}

/// Encode top-down RGBA8 rows. Straight (non-premultiplied) alpha expected.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let table = crc_table();
    let stride = width as usize * 4;
    // Filter type 1 (Sub) per row: cheap and compresses UI images well.
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks_exact(stride) {
        raw.push(1u8);
        for (i, &b) in row.iter().enumerate() {
            let left = if i >= 4 { row[i - 4] } else { 0 };
            raw.push(b.wrapping_sub(left));
        }
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, &table, b"IHDR", &ihdr);
    chunk(&mut out, &table, b"IDAT", &compressed);
    chunk(&mut out, &table, b"IEND", &[]);
    out
}

/// Write a PNG atomically (temporary file + rename).
pub fn write_file(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let data = encode_rgba(width, height, rgba);
    let tmp = path.with_extension("png.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_reference() {
        // CRC-32 of "IEND" is the well-known constant at the end of every PNG.
        assert_eq!(crc32(&crc_table(), &[b"IEND"]), 0xae42_6082);
    }

    #[test]
    fn roundtrip_through_inflate() {
        let (w, h) = (3u32, 2u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect();
        let png = encode_rgba(w, h, &px);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // Locate IDAT and inflate it, then undo the Sub filter.
        let idat = png.windows(4).position(|x| x == b"IDAT").unwrap();
        let len = u32::from_be_bytes(png[idat - 4..idat].try_into().unwrap()) as usize;
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[idat + 4..idat + 4 + len]).unwrap();
        let stride = w as usize * 4;
        let mut out = Vec::new();
        for row in raw.chunks_exact(stride + 1) {
            assert_eq!(row[0], 1);
            let mut line = vec![0u8; stride];
            for i in 0..stride {
                let left = if i >= 4 { line[i - 4] } else { 0 };
                line[i] = row[i + 1].wrapping_add(left);
            }
            out.extend(line);
        }
        assert_eq!(out, px);
    }
}
