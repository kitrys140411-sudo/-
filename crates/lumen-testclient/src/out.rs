//! The stdout event protocol: one line per event, made of space-separated `key=value`
//! tokens (the first token names the event), flushed immediately. Also the small pure
//! helpers that format protocol values for it.
//!
//! ```text
//! ready                                   first frame committed (and flushed)
//! configure w=W h=H states=S size=WxH serial=N
//!                                         w/h as sent by the compositor (0 = client's
//!                                         choice), size = the size actually drawn, states
//!                                         from maximized,fullscreen,activated,resizing,
//!                                         tiled_left,tiled_right,tiled_top,tiled_bottom,
//!                                         suspended (empty when none)
//! scale value=F                           fractional or integer scale changed (the integer
//!                                         scale is also reported once on the first configure)
//! enter kind=pointer x=F y=F surface=main|popup
//! leave kind=pointer surface=main|popup
//! motion x=F y=F surface=main|popup
//! button code=N state=pressed|released surface=main|popup
//! axis vertical=F horizontal=F surface=main|popup
//! cursor shape=NAME                       --cursor-shape applied on pointer enter
//! focus in|out surface=main|popup         keyboard focus (a popup grab moves it to the popup)
//! key sym=NAME utf8=TEXT state=pressed|released code=N
//! copy event=offered|send|cancelled [mime=M]
//! paste text=TEXT                         --paste, once per focus-in of the toplevel
//! paste error=no-selection|no-text-mime|no-data-device
//! popup event=open|configure|closed ...   --popup
//! frame seq=N presented_us=N refresh_ns=N flags=N [discarded=1]
//! closed                                  xdg_toplevel.close received (exits with 0)
//! exit reason=timeout|key                 --exit-after-ms / --close-on-key
//! ```
//!
//! TEXT is escaped with [`escape`] so it never contains whitespace.

use std::fmt::Write as _;
use std::io::Write as _;

/// Writes one event line to stdout and flushes it. Errors (e.g. a closed pipe) are ignored
/// so that a test harness that stops reading never makes the client panic.
pub fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// Escapes text so it stays a single whitespace-free token: backslash, whitespace and
/// control characters are written as escapes (`\\`, `\n`, `\r`, `\t`, `\xNN`, `\u{NNNN}`).
/// A space becomes `\x20`. Everything else, including non-ASCII text, is kept as is.
pub fn escape(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if c.is_whitespace() || c.is_control() => {
                if (c as u32) < 0x100 {
                    let _ = write!(s, "\\x{:02x}", c as u32);
                } else {
                    let _ = write!(s, "\\u{{{:04x}}}", c as u32);
                }
            }
            c => s.push(c),
        }
    }
    s
}

/// Names of the xdg_toplevel states, in a fixed order, joined with commas.
pub fn states_list(states: &[(bool, &str)]) -> String {
    states
        .iter()
        .filter(|(on, _)| *on)
        .map(|&(_, name)| name)
        .collect::<Vec<_>>()
        .join(",")
}

/// Converts an xkbcommon-style keysym name from the xkeysym tables (`XK_Escape`,
/// `XF86XK_AudioMute`) into the canonical libxkbcommon spelling (`Escape`, `XF86AudioMute`).
/// Keysyms without a name are spelled like xkb_keysym_get_name does: `U` plus the hex
/// code point for Unicode keysyms (`U0436`), `0x` plus 8 hex digits otherwise.
pub fn keysym_name(table_name: Option<&str>, raw: u32) -> String {
    match table_name {
        Some(name) => name.replacen("XK_", "", 1),
        None if (0x0100_0100..=0x0110_ffff).contains(&raw) => {
            let cp = raw & 0x00ff_ffff;
            if cp > 0xffff {
                format!("U{cp:08X}")
            } else {
                format!("U{cp:04X}")
            }
        }
        None => format!("0x{raw:08x}"),
    }
}

/// Mime types offered by `--copy`, best first.
pub const TEXT_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "text/plain"];

/// Picks the best text mime type of a selection offer for `--paste`.
pub fn pick_text_mime(offered: &[String]) -> Option<String> {
    const PREFERRED: [&str; 5] = [
        "text/plain;charset=utf-8",
        "UTF8_STRING",
        "text/plain",
        "TEXT",
        "STRING",
    ];
    PREFERRED
        .iter()
        .find_map(|want| offered.iter().find(|m| m.as_str() == *want))
        .or_else(|| offered.iter().find(|m| m.starts_with("text/plain")))
        .cloned()
}

/// Microseconds since the clock epoch from a wp_presentation_feedback.presented timestamp.
pub fn presented_us(tv_sec_hi: u32, tv_sec_lo: u32, tv_nsec: u32) -> u64 {
    let secs = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
    secs.saturating_mul(1_000_000)
        .saturating_add(u64::from(tv_nsec) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_basic() {
        assert_eq!(escape(""), "");
        assert_eq!(escape("abc"), "abc");
        assert_eq!(escape("hello world"), "hello\\x20world");
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("tab\there\r"), "tab\\there\\r");
        assert_eq!(escape("back\\slash"), "back\\\\slash");
        assert_eq!(escape("\u{1b}"), "\\x1b");
        assert_eq!(escape("\u{7f}"), "\\x7f");
        assert_eq!(escape("nb\u{a0}sp"), "nb\\xa0sp");
        assert_eq!(escape("em\u{2003}sp"), "em\\u{2003}sp");
        assert_eq!(escape("grüße 🙂"), "grüße\\x20🙂");
        assert_eq!(escape("k=v"), "k=v");
    }

    #[test]
    fn escaped_text_is_one_token() {
        let nasty = "a b\tc\nd\re\u{b}f\u{c}g\u{85}h\u{2028}i\u{3000}j";
        let escaped = escape(nasty);
        assert_eq!(escaped.split_whitespace().count(), 1);
        assert!(!escaped.chars().any(|c| c.is_control()));
    }

    #[test]
    fn states() {
        assert_eq!(states_list(&[]), "");
        assert_eq!(
            states_list(&[(false, "maximized"), (false, "activated")]),
            ""
        );
        assert_eq!(
            states_list(&[
                (true, "maximized"),
                (false, "fullscreen"),
                (true, "activated")
            ]),
            "maximized,activated"
        );
    }

    #[test]
    fn keysym_names() {
        assert_eq!(keysym_name(Some("XK_Escape"), 0xff1b), "Escape");
        assert_eq!(keysym_name(Some("XK_a"), 0x61), "a");
        assert_eq!(
            keysym_name(Some("XF86XK_AudioMute"), 0x1008ff12),
            "XF86AudioMute"
        );
        assert_eq!(keysym_name(None, 0x1234), "0x00001234");
        assert_eq!(keysym_name(None, 0x0100_0436), "U0436");
        assert_eq!(keysym_name(None, 0x0101_f642), "U0001F642");
    }

    #[test]
    fn text_mime() {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_text_mime(&v(&[])), None);
        assert_eq!(pick_text_mime(&v(&["image/png"])), None);
        assert_eq!(
            pick_text_mime(&v(&["text/plain", "text/plain;charset=utf-8"])).as_deref(),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            pick_text_mime(&v(&["STRING", "UTF8_STRING"])).as_deref(),
            Some("UTF8_STRING")
        );
        assert_eq!(
            pick_text_mime(&v(&["text/html", "text/plain;charset=iso-8859-1"])).as_deref(),
            Some("text/plain;charset=iso-8859-1")
        );
        assert_eq!(
            pick_text_mime(&v(&TEXT_MIME_TYPES)).as_deref(),
            Some(TEXT_MIME_TYPES[0])
        );
    }

    #[test]
    fn presentation_time() {
        assert_eq!(presented_us(0, 0, 0), 0);
        assert_eq!(presented_us(0, 2, 500_999), 2_000_500);
        assert_eq!(presented_us(1, 0, 0), (1u64 << 32) * 1_000_000);
    }
}
