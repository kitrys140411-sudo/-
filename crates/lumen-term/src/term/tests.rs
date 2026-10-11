use super::*;

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, 100)
}

fn feed(t: &mut Terminal, s: &str) {
    t.advance(s.as_bytes());
}

fn lines(t: &Terminal) -> Vec<String> {
    (0..t.term.rows()).map(|y| t.term.row_text(y)).collect()
}

fn cell(t: &Terminal, x: usize, y: usize) -> Cell {
    t.term.grid().row(y).cells[x]
}

#[test]
fn prints_and_wraps() {
    let mut t = term(5, 3);
    feed(&mut t, "hello world");
    assert_eq!(lines(&t), ["hello", " worl", "d"]);
    assert!(t.term.grid().row(0).wrapped);
    assert!(!t.term.grid().row(2).wrapped);
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (1, 2));
}

#[test]
fn pending_wrap_and_cr() {
    let mut t = term(5, 2);
    feed(&mut t, "abcde");
    assert_eq!((t.term.cursor.x, t.term.cursor.pending_wrap), (4, true));
    feed(&mut t, "\rX");
    assert_eq!(lines(&t), ["Xbcde", ""]);
}

#[test]
fn scrolls_into_history() {
    let mut t = term(4, 2);
    feed(&mut t, "1\r\n2\r\n3\r\n4");
    assert_eq!(lines(&t), ["3", "4"]);
    let g = t.term.grid();
    assert_eq!(g.history(), 2);
    assert_eq!(g.line(0).unwrap().text(0, 1), "1");
    assert_eq!(t.term.scrolled, 2);
}

#[test]
fn cursor_movement_and_erase() {
    let mut t = term(10, 3);
    feed(&mut t, "abcdefghij\x1b[2;3Hxyz\x1b[1;5H\x1b[K");
    assert_eq!(lines(&t), ["abcd", "  xyz", ""]);
    feed(&mut t, "\x1b[2;4H\x1b[1K");
    assert_eq!(lines(&t), ["abcd", "    z", ""]);
    feed(&mut t, "\x1b[2J");
    assert_eq!(lines(&t), ["", "", ""]);
    // CUU/CUD/CUF/CUB clamp at the edges.
    feed(&mut t, "\x1b[H\x1b[5A\x1b[20C\x1b[9B");
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (9, 2));
}

#[test]
fn insert_delete_chars_and_lines() {
    let mut t = term(8, 4);
    feed(&mut t, "abcdef\x1b[1;3H\x1b[2@");
    assert_eq!(t.term.row_text(0), "ab  cdef");
    feed(&mut t, "\x1b[3P");
    assert_eq!(t.term.row_text(0), "abdef");
    feed(&mut t, "\x1b[1;2H\x1b[2X");
    assert_eq!(t.term.row_text(0), "a  ef");
    feed(&mut t, "\x1b[2;1Hline2\x1b[3;1Hline3\x1b[2;1H\x1b[L");
    assert_eq!(lines(&t), ["a  ef", "", "line2", "line3"]);
    feed(&mut t, "\x1b[2M");
    assert_eq!(lines(&t), ["a  ef", "line3", "", ""]);
}

#[test]
fn scroll_region() {
    let mut t = term(4, 5);
    feed(&mut t, "1\r\n2\r\n3\r\n4\r\n5");
    feed(&mut t, "\x1b[2;4r");
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (0, 0));
    feed(&mut t, "\x1b[4;1H\n");
    assert_eq!(lines(&t), ["1", "3", "4", "", "5"]);
    // Region scrolls do not feed the history.
    assert_eq!(t.term.grid().history(), 0);
    // Reverse index at the top margin scrolls down.
    feed(&mut t, "\x1b[2;1H\x1bM");
    assert_eq!(lines(&t), ["1", "", "3", "4", "5"]);
    // SU/SD.
    feed(&mut t, "\x1b[S");
    assert_eq!(lines(&t), ["1", "3", "4", "", "5"]);
}

#[test]
fn origin_mode() {
    let mut t = term(4, 5);
    feed(&mut t, "\x1b[2;4r\x1b[?6h\x1b[1;1HX\x1b[9;1HY");
    assert_eq!(lines(&t), ["", "X", "", "Y", ""]);
    feed(&mut t, "\x1b[6n");
    assert_eq!(t.term.replies, b"\x1b[3;2R");
}

#[test]
fn sgr_attributes_and_colors() {
    let mut t = term(20, 1);
    feed(&mut t, "\x1b[1;3;4;31;42ma\x1b[22;23;24;39;49mb\x1b[38;5;200;48;2;1;2;3mc\x1b[38:2::10:20:30;4:3md\x1b[0;7;9;53me");
    let a = cell(&t, 0, 0);
    assert!(a.has(attr::BOLD) && a.has(attr::ITALIC));
    assert_eq!((a.underline, a.fg, a.bg), (Underline::Single, Color::Indexed(1), Color::Indexed(2)));
    let b = cell(&t, 1, 0);
    assert_eq!((b.attrs & attr::STYLE, b.underline, b.fg, b.bg), (0, Underline::None, Color::Default, Color::Default));
    let c = cell(&t, 2, 0);
    assert_eq!((c.fg, c.bg), (Color::Indexed(200), Color::Rgb(1, 2, 3)));
    let d = cell(&t, 3, 0);
    assert_eq!((d.fg, d.underline), (Color::Rgb(10, 20, 30), Underline::Curly));
    let e = cell(&t, 4, 0);
    assert!(e.has(attr::INVERSE) && e.has(attr::STRIKE) && e.has(attr::OVERLINE));
    assert_eq!(e.fg, Color::Default);
    feed(&mut t, "\x1b[0;95;105mf");
    assert_eq!((cell(&t, 5, 0).fg, cell(&t, 5, 0).bg), (Color::Indexed(13), Color::Indexed(13)));
}

#[test]
fn erase_uses_background_color() {
    let mut t = term(4, 2);
    feed(&mut t, "\x1b[44m\x1b[2J");
    assert_eq!(cell(&t, 3, 1).bg, Color::Indexed(4));
}

#[test]
fn wide_characters() {
    let mut t = term(5, 2);
    feed(&mut t, "a日本");
    assert_eq!(t.term.row_text(0), "a日本");
    assert!(cell(&t, 1, 0).has(attr::WIDE) && cell(&t, 2, 0).has(attr::SPACER));
    assert_eq!(t.term.cursor.x, 4);
    assert!(t.term.cursor.pending_wrap);
    // A wide char at the last column wraps first.
    let mut t = term(4, 2);
    feed(&mut t, "abc日");
    assert_eq!(lines(&t), ["abc", "日"]);
    // Overwriting half of a wide char clears the other half.
    let mut t = term(4, 1);
    feed(&mut t, "日本\x1b[1;2Hx");
    assert_eq!(t.term.row_text(0), " x本");
    assert!(!cell(&t, 0, 0).has(attr::WIDE));
}

#[test]
fn combining_marks_and_emoji_sequences() {
    let mut t = term(10, 1);
    feed(&mut t, "e\u{301}x");
    assert_eq!(t.term.row_text(0), "e\u{301}x");
    assert_eq!(t.term.cursor.x, 2);
    // Family emoji: one grapheme in one wide cell.
    feed(&mut t, "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}!");
    assert_eq!(t.term.row_text(0), "e\u{301}x\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}!");
    assert_eq!(t.term.cursor.x, 5);
}

#[test]
fn flags_are_one_wide_cluster() {
    let mut t = term(10, 1);
    feed(&mut t, "\u{1F1FA}\u{1F1E6}x\u{1F1E9}");
    assert_eq!(t.term.row_text(0), "\u{1F1FA}\u{1F1E6}x\u{1F1E9}");
    assert!(cell(&t, 0, 0).has(attr::WIDE));
    assert_eq!(cell(&t, 2, 0).ch, 'x');
    assert_eq!(t.term.cursor.x, 5);
}

#[test]
fn tabs() {
    let mut t = term(20, 1);
    feed(&mut t, "a\tb\tc");
    assert_eq!(t.term.row_text(0), "a       b       c");
    feed(&mut t, "\x1b[3g\r\x1b[5C\x1bH\r\tX");
    assert_eq!(t.term.cursor.x, 6);
    feed(&mut t, "\x1b[Z");
    assert_eq!(t.term.cursor.x, 5);
}

#[test]
fn alternate_screen_saves_and_restores() {
    let mut t = term(6, 2);
    // 1049 keeps the cursor where it was (xterm), so home it first.
    feed(&mut t, "main\x1b[?1049h\x1b[HALT");
    assert!(t.term.alt_active);
    assert_eq!(lines(&t), ["ALT", ""]);
    feed(&mut t, "\x1b[?1049l");
    assert_eq!(lines(&t), ["main", ""]);
    assert_eq!(t.term.cursor.x, 4);
    // The alternate screen never feeds the history.
    feed(&mut t, "\x1b[?1049h1\r\n2\r\n3\r\n4");
    assert_eq!(t.term.grid().history(), 0);
}

#[test]
fn modes_and_reports() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[?1h\x1b[?2004h\x1b[?1006h\x1b[?1002h\x1b[?25l\x1b[4h");
    let m = t.term.modes;
    assert!(m.app_cursor && m.bracketed_paste && !m.show_cursor && m.insert);
    assert_eq!((m.mouse, m.mouse_encoding), (MouseMode::Button, MouseEncoding::Sgr));
    feed(&mut t, "\x1b[?2004$p\x1b[c\x1b[>c\x1b[5n\x1b[18t");
    let r = String::from_utf8(std::mem::take(&mut t.term.replies)).unwrap();
    assert_eq!(r, "\x1b[?2004;1$y\x1b[?62;22c\x1b[>1;10;0c\x1b[0n\x1b[8;3;10t");
    feed(&mut t, "\x1b[3 q");
    assert_eq!((t.term.cursor_shape, t.term.modes.cursor_blink), (CursorShape::Underline, true));
}

#[test]
fn osc_title_colors_links_clipboard() {
    let mut t = term(10, 1);
    feed(&mut t, "\x1b]2;my; title\x07");
    assert_eq!(t.term.title, "my; title");
    feed(&mut t, "\x1b]4;1;rgb:ff/00/80\x1b\\\x1b]4;1;?\x07");
    assert_eq!(t.term.color(1), (255, 0, 128));
    assert_eq!(t.term.replies, b"\x1b]4;1;rgb:ffff/0000/8080\x07");
    t.term.replies.clear();
    feed(&mut t, "\x1b]11;#102030\x07\x1b]11;?\x1b\\");
    assert_eq!(t.term.bg(), (0x10, 0x20, 0x30));
    assert_eq!(t.term.replies, b"\x1b]11;rgb:1010/2020/3030\x1b\\");
    feed(&mut t, "\x1b]8;;https://example.com\x07link\x1b]8;;\x07x");
    assert_eq!(cell(&t, 0, 0).link, 1);
    assert_eq!(cell(&t, 4, 0).link, 0);
    assert_eq!(t.term.links[0], "https://example.com");
    feed(&mut t, "\x1b]52;c;aGVsbG8=\x07");
    assert!(t.term.events.contains(&Event::ClipboardSet('c', "hello".into())));
}

#[test]
fn charsets_line_drawing() {
    let mut t = term(10, 1);
    feed(&mut t, "\x1b(0lqqk\x1b(Bx");
    assert_eq!(t.term.row_text(0), "┌──┐x");
    // SO/SI switch to G1.
    feed(&mut t, "\r\x1b)0\x0ex\x0fx");
    assert_eq!(t.term.row_text(0), "│x─┐x");
}

#[test]
fn decaln_and_reset() {
    let mut t = term(3, 2);
    feed(&mut t, "\x1b#8");
    assert_eq!(lines(&t), ["EEE", "EEE"]);
    feed(&mut t, "\x1b[1;31m\x1bc");
    assert_eq!(lines(&t), ["", ""]);
    assert_eq!(t.term.cursor.pen, Cell::default());
}

#[test]
fn repeat_and_decrqss() {
    let mut t = term(10, 1);
    feed(&mut t, "x\x1b[4b");
    assert_eq!(t.term.row_text(0), "xxxxx");
    feed(&mut t, "\x1b[1;32m\x1bP$qm\x1b\\");
    assert_eq!(t.term.replies, b"\x1bP1$r0;1;32m\x1b\\");
}

#[test]
fn save_restore_cursor_with_attributes() {
    let mut t = term(10, 3);
    feed(&mut t, "\x1b[2;3H\x1b[1m\x1b7\x1b[H\x1b[0mz\x1b8y");
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (3, 1));
    assert!(cell(&t, 2, 1).has(attr::BOLD));
}

#[test]
fn resize_reflows_wrapped_lines() {
    let mut t = term(10, 4);
    feed(&mut t, "0123456789abcdef\r\nshort");
    assert_eq!(lines(&t), ["0123456789", "abcdef", "short", ""]);
    t.term.resize(6, 4);
    assert_eq!(lines(&t), ["012345", "6789ab", "cdef", "short"]);
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (5, 3));
    t.term.resize(20, 4);
    assert_eq!(lines(&t), ["0123456789abcdef", "short", "", ""]);
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (5, 1));
}

#[test]
fn resize_height_uses_history() {
    let mut t = term(5, 3);
    feed(&mut t, "1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(lines(&t), ["3", "4", "5"]);
    t.term.resize(5, 5);
    assert_eq!(lines(&t), ["1", "2", "3", "4", "5"]);
    assert_eq!(t.term.cursor.y, 4);
    t.term.resize(5, 2);
    assert_eq!(lines(&t), ["4", "5"]);
    assert_eq!(t.term.cursor.y, 1);
}

#[test]
fn reflow_keeps_wide_chars_whole() {
    let mut t = term(6, 2);
    feed(&mut t, "ab日本語");
    t.term.resize(5, 3);
    assert_eq!(lines(&t), ["ab日", "本語", ""]);
}

#[test]
fn history_is_bounded() {
    let mut t = Terminal::new(4, 2, 10);
    for i in 0..100 {
        feed(&mut t, &format!("{i}\r\n"));
    }
    assert_eq!(t.term.grid().history(), 10);
    assert_eq!(t.term.grid().dropped, 89);
}

#[test]
fn utf8_split_across_reads() {
    let mut t = term(5, 1);
    let bytes = "é日".as_bytes();
    for b in bytes {
        t.advance(std::slice::from_ref(b));
    }
    assert_eq!(t.term.row_text(0), "é日");
}

#[test]
fn color_spec_parsing() {
    use super::osc::parse_color;
    assert_eq!(parse_color("rgb:f/8/0"), Some((255, 136, 0)));
    assert_eq!(parse_color("rgb:ffff/8080/0000"), Some((255, 128, 0)));
    assert_eq!(parse_color("#fff"), Some((255, 255, 255)));
    assert_eq!(parse_color("#123456"), Some((0x12, 0x34, 0x56)));
    assert_eq!(parse_color("bogus"), None);
    let enc = super::osc::base64_encode(b"hello world!");
    assert_eq!(super::osc::base64_decode(enc.as_bytes()).unwrap(), b"hello world!");
}
