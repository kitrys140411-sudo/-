//! Operating system commands: titles, palette and dynamic colors, hyperlinks,
//! clipboard.

use super::{Event, Term};

/// Parse an X11 color spec: `rgb:r/g/b` (1–4 hex digits per channel) or
/// `#rgb`, `#rrggbb`, `#rrrgggbbb`, `#rrrrggggbbbb`.
pub fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let scale = |h: &str| -> Option<u8> {
        if h.is_empty() || h.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    if let Some(rest) = s.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let (r, g, b) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() {
            return None;
        }
        return Some((scale(r)?, scale(g)?, scale(b)?));
    }
    let hex = s.strip_prefix('#')?;
    if hex.is_empty() || hex.len() % 3 != 0 || hex.len() > 12 {
        return None;
    }
    let n = hex.len() / 3;
    // `#rgb` style specs are the high digits of each channel.
    let chan = |i: usize| -> Option<u8> {
        let h = &hex[i * n..(i + 1) * n];
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(match n {
            1 => (v * 17) as u8,
            2 => v as u8,
            3 => (v >> 4) as u8,
            _ => (v >> 8) as u8,
        })
    };
    Some((chan(0)?, chan(1)?, chan(2)?))
}

fn color_spec((r, g, b): (u8, u8, u8)) -> String {
    format!("rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}", r, r, g, g, b, b)
}

pub fn base64_decode(s: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let data: Vec<u8> = s.iter().copied().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    for chunk in data.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        let bytes = [(acc >> 16) as u8, (acc >> 8) as u8, acc as u8];
        let n = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        out.extend_from_slice(&bytes[..n]);
    }
    Some(out)
}

pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[v as usize & 63] as char } else { '=' });
    }
    out
}

pub fn dispatch(t: &mut Term, params: &[&[u8]], bell: bool) {
    let Some(cmd) = params.first() else { return };
    let term = if bell { "\x07" } else { "\x1b\\" };
    let text = |i: usize| params.get(i).map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_default();
    match *cmd {
        b"0" | b"2" => {
            // Titles may contain ';'.
            let title: Vec<String> = (1..params.len()).map(text).collect();
            let title = title.join(";");
            let title: String = title.chars().filter(|c| !c.is_control()).take(256).collect();
            t.title = title.clone();
            t.events.push(Event::Title(title));
        }
        b"4" => {
            let mut i = 1;
            while i + 1 < params.len() {
                let idx = text(i).parse::<usize>().ok().filter(|&n| n < 256);
                let spec = text(i + 1);
                if let Some(n) = idx {
                    if spec == "?" {
                        let s = format!("\x1b]4;{n};{}{term}", color_spec(t.color(n as u8)));
                        t.replies.extend_from_slice(s.as_bytes());
                    } else if let Some(c) = parse_color(&spec) {
                        t.palette[n] = Some(c);
                    }
                }
                i += 2;
            }
            t.changed = true;
        }
        b"8" => {
            // OSC 8 ; params ; URI — an empty URI ends the link.
            let uri = text(2);
            if uri.is_empty() {
                t.current_link = 0;
            } else {
                let id = match t.links.iter().position(|l| *l == uri) {
                    Some(i) => i + 1,
                    None => {
                        if t.links.len() >= u16::MAX as usize - 1 {
                            t.links.clear();
                        }
                        t.links.push(uri);
                        t.links.len()
                    }
                };
                t.current_link = id as u16;
            }
        }
        b"10" | b"11" | b"12" => {
            // OSC 10;fg;bg;cursor: each further parameter is the next color.
            let first = match *cmd {
                b"10" => 10,
                b"11" => 11,
                _ => 12,
            };
            for (k, i) in (1..params.len()).enumerate() {
                let which = first + k;
                let spec = text(i);
                if spec == "?" {
                    let c = match which {
                        10 => t.fg(),
                        11 => t.bg(),
                        _ => t.cursor_color(),
                    };
                    let s = format!("\x1b]{which};{}{term}", color_spec(c));
                    t.replies.extend_from_slice(s.as_bytes());
                } else if let Some(c) = parse_color(&spec) {
                    match which {
                        10 => t.fg_override = Some(c),
                        11 => t.bg_override = Some(c),
                        12 => t.cursor_override = Some(c),
                        _ => {}
                    }
                }
            }
            t.changed = true;
        }
        b"52" => {
            let target = text(1).chars().next().unwrap_or('c');
            let target = if target == 'p' || target == 's' { 'p' } else { 'c' };
            let data = params.get(2).copied().unwrap_or(b"");
            if data == b"?" {
                t.events.push(Event::ClipboardQuery(target));
            } else if let Some(bytes) = base64_decode(data) {
                // Bound the size an application can push (1 MiB).
                if bytes.len() <= 1 << 20 {
                    t.events.push(Event::ClipboardSet(target, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        b"104" => {
            if params.len() <= 1 {
                t.palette = [None; 256];
            } else {
                for i in 1..params.len() {
                    if let Ok(n) = text(i).parse::<usize>() {
                        if n < 256 {
                            t.palette[n] = None;
                        }
                    }
                }
            }
            t.changed = true;
        }
        b"110" => t.fg_override = None,
        b"111" => t.bg_override = None,
        b"112" => t.cursor_override = None,
        _ => {}
    }
}

impl Term {
    /// Answer an OSC 52 query with the clipboard contents.
    pub fn clipboard_reply(&mut self, target: char, text: &str) {
        let s = format!("\x1b]52;{target};{}\x07", base64_encode(text.as_bytes()));
        self.replies.extend_from_slice(s.as_bytes());
    }
}
