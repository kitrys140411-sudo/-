//! Minimal PNG encoder for screenshots (RGBA8, non-interlaced) and a decoder
//! for application icons (every color type and bit depth, interlaced or not),
//! plus the resampler that fits decoded icons to a pixel size.

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

/// Images larger than this (in pixels) are refused: icons are small, and a
/// broken or hostile file must not make us allocate gigabytes.
const MAX_PIXELS: u64 = 4096 * 4096;

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Undo the per-row filters of one (sub)image in place: `data` holds rows of
/// a filter byte plus `stride` bytes; returns the unfiltered rows.
fn unfilter(data: &[u8], stride: usize, rows: usize, bpp: usize) -> Result<Vec<u8>, String> {
    if data.len() < (stride + 1) * rows {
        return Err("image data is truncated".into());
    }
    let mut out = vec![0u8; stride * rows];
    for r in 0..rows {
        let src = &data[r * (stride + 1)..(r + 1) * (stride + 1)];
        let (prev, cur) = out.split_at_mut(r * stride);
        let prev = if r > 0 { &prev[(r - 1) * stride..] } else { &[][..] };
        let cur = &mut cur[..stride];
        let up = |i: usize| if r > 0 { prev[i] } else { 0 };
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = up(i);
            let c = if i >= bpp { up(i - bpp) } else { 0 };
            let x = src[i + 1];
            cur[i] = match src[0] {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => x.wrapping_add(paeth(a, b, c)),
                f => return Err(format!("bad filter type {f}")),
            };
        }
    }
    Ok(out)
}

/// Decode a PNG into top-down RGBA8 with straight alpha.
pub fn decode(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a PNG file".into());
    }
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    let mut pos = 8;
    loop {
        let header = data.get(pos..pos + 8).ok_or("truncated chunk")?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let body = data.get(pos + 8..pos + 8 + len).ok_or("truncated chunk")?;
        pos += 12 + len;
        match &kind {
            b"IHDR" if len == 13 => {
                w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                (depth, ctype, interlace) = (body[8], body[9], body[12]);
            }
            b"PLTE" => palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
    }
    let channels: u32 = match (ctype, depth) {
        (0, 1 | 2 | 4 | 8 | 16) => 1,
        (2, 8 | 16) => 3,
        (3, 1 | 2 | 4 | 8) => 1,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(format!("unsupported color type {ctype} with bit depth {depth}")),
    };
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS || interlace > 1 {
        return Err(format!("unsupported image {w}x{h} (interlace {interlace})"));
    }
    let bits = channels * depth as u32;
    let bpp = (bits as usize).div_ceil(8);
    // Adam7 passes as (x0, y0, dx, dy); a plain image is one pass.
    let passes: &[(u32, u32, u32, u32)] = if interlace == 1 {
        &[(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    } else {
        &[(0, 0, 1, 1)]
    };
    let mut expected = 0usize;
    for &(x0, y0, dx, dy) in passes {
        let (pw, ph) = ((w.saturating_sub(x0)).div_ceil(dx), (h.saturating_sub(y0)).div_ceil(dy));
        if pw > 0 && ph > 0 {
            expected += ((pw as usize * bits as usize).div_ceil(8) + 1) * ph as usize;
        }
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, expected).map_err(|e| format!("bad image data: {e:?}"))?;
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let max = ((1u32 << depth.min(8)) - 1) as u32;
    let mut offset = 0;
    for &(x0, y0, dx, dy) in passes {
        let (pw, ph) = ((w.saturating_sub(x0)).div_ceil(dx), (h.saturating_sub(y0)).div_ceil(dy));
        if pw == 0 || ph == 0 {
            continue;
        }
        let stride = (pw as usize * bits as usize).div_ceil(8);
        let rows = unfilter(&raw[offset.min(raw.len())..], stride, ph as usize, bpp)?;
        offset += (stride + 1) * ph as usize;
        for r in 0..ph as usize {
            let row = &rows[r * stride..(r + 1) * stride];
            // Sample `i` of the row at full precision (16-bit kept whole for tRNS).
            let sample = |i: usize| -> u32 {
                match depth {
                    16 => u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]) as u32,
                    8 => row[i] as u32,
                    d => {
                        let bit = i * d as usize;
                        ((row[bit / 8] >> (8 - d as usize - bit % 8)) as u32) & max
                    }
                }
            };
            let to8 = |v: u32| -> u8 {
                match depth {
                    16 => (v >> 8) as u8,
                    8 => v as u8,
                    _ => (v * 255 / max) as u8,
                }
            };
            let trns16 = |k: usize| trns.get(k * 2..k * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32);
            for c in 0..pw as usize {
                let n = channels as usize;
                let px = match ctype {
                    0 => {
                        let g = sample(c);
                        let a = if trns16(0) == Some(g) { 0 } else { 255 };
                        [to8(g), to8(g), to8(g), a]
                    }
                    2 => {
                        let (r, g, b) = (sample(c * 3), sample(c * 3 + 1), sample(c * 3 + 2));
                        let a = if (trns16(0), trns16(1), trns16(2)) == (Some(r), Some(g), Some(b)) { 0 } else { 255 };
                        [to8(r), to8(g), to8(b), a]
                    }
                    3 => {
                        let i = sample(c) as usize;
                        let p = palette.get(i).ok_or("palette index out of range")?;
                        [p[0], p[1], p[2], trns.get(i).copied().unwrap_or(255)]
                    }
                    4 => {
                        let (g, a) = (to8(sample(c * n)), to8(sample(c * n + 1)));
                        [g, g, g, a]
                    }
                    _ => [to8(sample(c * n)), to8(sample(c * n + 1)), to8(sample(c * n + 2)), to8(sample(c * n + 3))],
                };
                let (x, y) = (x0 + c as u32 * dx, y0 + r as u32 * dy);
                let o = (y as usize * w as usize + x as usize) * 4;
                out[o..o + 4].copy_from_slice(&px);
            }
        }
    }
    Ok((w, h, out))
}

/// Tent-filter weights mapping `src` samples onto `dst` (bilinear when
/// enlarging, an area average when shrinking): per output index, the first
/// source index and its weights.
fn weights(src: u32, dst: u32) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let support = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale - 0.5;
            let lo = (center - support).floor().max(0.0) as usize;
            let hi = ((center + support).ceil() as usize).min(src as usize - 1);
            let mut ws: Vec<f32> = (lo..=hi).map(|j| (1.0 - (j as f32 - center).abs() / support).max(0.0)).collect();
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                ws.iter_mut().for_each(|w| *w /= sum);
            } else {
                ws = vec![1.0];
            }
            (lo, ws)
        })
        .collect()
}

/// Resample straight-alpha RGBA to `dw` x `dh`, returning premultiplied RGBA
/// (what the renderer draws); filtering happens premultiplied so transparent
/// pixels do not darken the edges.
pub fn resize_premultiplied(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let pre: Vec<f32> = src
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        })
        .collect();
    let (wx, wy) = (weights(sw, dw), weights(sh, dh));
    // Horizontal pass into sw→dw columns, then vertical.
    let mut tmp = vec![0f32; dw as usize * sh as usize * 4];
    for y in 0..sh as usize {
        for (x, (lo, ws)) in wx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let s = (y * sw as usize + lo + k) * 4;
                for c in 0..4 {
                    acc[c] += pre[s + c] * w;
                }
            }
            tmp[(y * dw as usize + x) * 4..][..4].copy_from_slice(&acc);
        }
    }
    let mut out = vec![0u8; dw as usize * dh as usize * 4];
    for (y, (lo, ws)) in wy.iter().enumerate() {
        for x in 0..dw as usize {
            let mut acc = [0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let s = ((lo + k) * dw as usize + x) * 4;
                for c in 0..4 {
                    acc[c] += tmp[s + c] * w;
                }
            }
            let a = acc[3].round().clamp(0.0, 255.0);
            let o = (y * dw as usize + x) * 4;
            for c in 0..3 {
                out[o + c] = acc[c].round().clamp(0.0, a) as u8;
            }
            out[o + 3] = a as u8;
        }
    }
    out
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

    /// A PNG built by hand: `rows` are unfiltered scanlines (filter byte 0 added).
    fn handmade(w: u32, h: u32, depth: u8, ctype: u8, interlace: u8, extra: &[(&[u8; 4], Vec<u8>)], raw: Vec<u8>) -> Vec<u8> {
        let table = crc_table();
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, ctype, 0, 0, interlace]);
        chunk(&mut out, &table, b"IHDR", &ihdr);
        for (kind, data) in extra {
            chunk(&mut out, &table, kind, data);
        }
        chunk(&mut out, &table, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
        chunk(&mut out, &table, b"IEND", &[]);
        out
    }

    #[test]
    fn decodes_what_we_encode() {
        let (w, h) = (5u32, 3u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 53 % 256) as u8).collect();
        assert_eq!(decode(&encode_rgba(w, h, &px)).unwrap(), (w, h, px));
        assert!(decode(b"GIF89a").is_err());
        assert!(decode(&encode_rgba(w, h, &vec![0; (w * h * 4) as usize])[..40]).is_err());
    }

    #[test]
    fn decodes_palette_gray_and_rgb16() {
        // 2-bit palette with transparency: indices 0,1,2,3 in one row.
        let plte = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9];
        let png = handmade(4, 1, 2, 3, 0, &[(b"PLTE", plte), (b"tRNS", vec![255, 128])], vec![0, 0b00_01_10_11]);
        let (_, _, px) = decode(&png).unwrap();
        assert_eq!(px, vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 9, 9, 9, 255]);
        // 4-bit gray with a transparent value.
        let png = handmade(2, 1, 4, 0, 0, &[(b"tRNS", vec![0, 15])], vec![0, 0x0f]);
        assert_eq!(decode(&png).unwrap().2, vec![0, 0, 0, 255, 255, 255, 255, 0]);
        // 16-bit RGB keeps the high bytes; Up filter on the second row.
        let png = handmade(1, 2, 16, 2, 0, &[], vec![0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 2, 1, 0, 1, 0, 1, 0]);
        assert_eq!(decode(&png).unwrap().2, vec![0x12, 0x56, 0x9a, 255, 0x13, 0x57, 0x9b, 255]);
    }

    #[test]
    fn decodes_adam7() {
        // 3x3 gray, interlaced: pass 1 has (0,0), pass 4 (2,0), pass 5 (0,2) and (2,2),
        // pass 6 (1,0), (1,2), pass 7 the middle row.
        let raw = vec![0, 10, /* p4 */ 0, 30, /* p5 */ 0, 70, 90, /* p6 */ 0, 20, 0, 80, /* p7 */ 0, 40, 50, 60];
        let png = handmade(3, 3, 8, 0, 1, &[], raw);
        let gray: Vec<u8> = decode(&png).unwrap().2.chunks(4).map(|p| p[0]).collect();
        assert_eq!(gray, vec![10, 20, 30, 40, 50, 60, 70, 80, 90]);
    }

    #[test]
    fn resizing_keeps_color_and_premultiplies() {
        // A 4x4 opaque red square shrinks to 2x2 red, a half-transparent white to premultiplied gray.
        let red: Vec<u8> = [255, 0, 0, 255].repeat(16);
        assert_eq!(resize_premultiplied(&red, 4, 4, 2, 2), [255, 0, 0, 255].repeat(4));
        let half: Vec<u8> = [255, 255, 255, 128].repeat(4);
        assert_eq!(resize_premultiplied(&half, 2, 2, 3, 3), [128, 128, 128, 128].repeat(9));
        // Transparent pixels next to opaque ones do not darken them.
        let edge = [[0, 0, 0, 0], [255, 255, 255, 255]].concat();
        let out = resize_premultiplied(&edge, 2, 1, 1, 1);
        assert_eq!((out[0], out[3]), (128, 128));
    }
}
