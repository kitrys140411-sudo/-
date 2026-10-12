//! VT100/VT220 conformance scenarios in the spirit of vttest's screens
//! (cursor movement, wrapping, scrolling regions, editing, tabs, character
//! sets, reports), each checked against the exact expected screen.

use super::*;

fn screen(cols: usize, rows: usize, input: &str) -> (Vec<String>, Terminal) {
    let mut t = Terminal::new(cols, rows, 0);
    t.advance(input.as_bytes());
    let lines = (0..rows)
        .map(|y| {
            let r = t.term.grid().row(y);
            r.text(0, r.cells.len())
        })
        .collect();
    (lines, t)
}

fn pad(lines: &[&str], cols: usize) -> Vec<String> {
    lines.iter().map(|l| format!("{l:cols$}")).collect()
}

#[test]
fn box_drawn_with_cursor_movements() {
    // Fill with E, erase the inside with ED/EL from both ends, then draw a
    // frame of '*' using absolute and relative moves.
    let input = concat!(
        "\x1b#8",
        "\x1b[2;2H\x1b[K\x1b[3;2H\x1b[K\x1b[4;2H\x1b[K",
        "\x1b[1;1H*\x1b[1;10H*",
        "\x1b[5;1H*\x1b[5;10H*",
        "\x1b[2;1H*\x1b[2;10H*\x1b[3;1H*\x1b[3;10H*\x1b[4;1H*\x1b[4;10H*",
        "\x1b[1;2H\x1b[8C\x1b[D\x1b[D+",
        "\x1b[5;9H\x1b[A\x1b[2D+",
    );
    let (s, _) = screen(10, 5, input);
    assert_eq!(s, pad(&["*EEEEEE+E*", "*        *", "*        *", "*     +  *", "*EEEEEEEE*"], 10));
}

#[test]
fn autowrap_on_and_off() {
    let (s, _) = screen(10, 3, "ABCDEFGHIJKLMNOPQRSTUVWXY");
    assert_eq!(s, pad(&["ABCDEFGHIJ", "KLMNOPQRST", "UVWXY"], 10));
    let (s, t) = screen(10, 3, "\x1b[?7lABCDEFGHIJKLMNO");
    assert_eq!(s, pad(&["ABCDEFGHIO", "", ""], 10));
    assert_eq!(t.term.cursor.x, 9);
}

#[test]
fn cursor_stops_at_margins_and_edges() {
    let (_, t) = screen(10, 6, "\x1b[2;5r\x1b[3;3H\x1b[20A");
    assert_eq!(t.term.cursor.y, 1, "CUU stops at the top margin");
    let (_, t) = screen(10, 6, "\x1b[2;5r\x1b[3;3H\x1b[20B");
    assert_eq!(t.term.cursor.y, 4, "CUD stops at the bottom margin");
    let (_, t) = screen(10, 6, "\x1b[2;5r\x1b[6;3H\x1b[20B");
    assert_eq!(t.term.cursor.y, 5, "below the region: stops at the screen edge");
    let (_, t) = screen(10, 6, "\x1b[1;1H\x1b[5D\x1b[1;10H\x1b[5C");
    assert_eq!(t.term.cursor.x, 9);
}

#[test]
fn scrolling_region_index_and_reverse_index() {
    // Lines 1..6, region 2..5: IND at the bottom margin scrolls only it.
    let fill = "1\r\n2\r\n3\r\n4\r\n5\r\n6";
    let (s, _) = screen(4, 6, &format!("{fill}\x1b[2;5r\x1b[5;1H\x1bD\x1bD"));
    assert_eq!(s, pad(&["1", "4", "5", "", "", "6"], 4));
    let (s, _) = screen(4, 6, &format!("{fill}\x1b[2;5r\x1b[2;1H\x1bM"));
    assert_eq!(s, pad(&["1", "", "2", "3", "4", "6"], 4));
    // NEL = CR + IND.
    let (_, t) = screen(4, 6, "ab\x1bEc");
    assert_eq!((t.term.cursor.x, t.term.cursor.y), (1, 1));
}

#[test]
fn origin_mode_and_saved_cursor() {
    let (s, t) = screen(6, 6, "\x1b[3;5r\x1b[?6h\x1b[1;1HA\x1b[99;1HB\x1b7\x1b[?6l\x1b[1;1HC\x1b8D");
    assert_eq!(s, pad(&["C", "", "A", "", "BD", ""], 6));
    assert!(t.term.modes.origin, "DECRC restores origin mode");
}

#[test]
fn insert_delete_characters_and_lines() {
    let (s, _) = screen(8, 4, "ABCDEFGH\x1b[1;3H\x1b[2@xy");
    assert_eq!(s[0], "ABxyCDEF");
    let (s, _) = screen(8, 4, "ABCDEFGH\x1b[1;3H\x1b[3P");
    assert_eq!(s[0], "ABFGH   ");
    let (s, _) = screen(8, 4, "\x1b[4h\x1b[1;1HABC\x1b[1;1Hxy\x1b[4l");
    assert_eq!(s[0], "xyABC   ", "insert mode shifts");
    let fill = "1\r\n2\r\n3\r\n4";
    let (s, _) = screen(4, 4, &format!("{fill}\x1b[2;3r\x1b[2;1H\x1b[L"));
    assert_eq!(s, pad(&["1", "", "2", "4"], 4), "IL inside the region");
    let (s, _) = screen(4, 4, &format!("{fill}\x1b[2;3r\x1b[2;1H\x1b[M"));
    assert_eq!(s, pad(&["1", "3", "", "4"], 4), "DL inside the region");
}

#[test]
fn erase_variants() {
    let (s, _) = screen(6, 3, "\x1b#8\x1b[2;3H\x1b[1K");
    assert_eq!(s, pad(&["EEEEEE", "   EEE", "EEEEEE"], 6));
    let (s, _) = screen(6, 3, "\x1b#8\x1b[2;3H\x1b[1J");
    assert_eq!(s, pad(&["", "   EEE", "EEEEEE"], 6));
    let (s, _) = screen(6, 3, "\x1b#8\x1b[2;3H\x1b[J");
    assert_eq!(s, pad(&["EEEEEE", "EE", ""], 6));
    let (s, _) = screen(6, 3, "\x1b#8\x1b[2;3H\x1b[2X");
    assert_eq!(s, pad(&["EEEEEE", "EE  EE", "EEEEEE"], 6));
}

#[test]
fn tab_stops_set_and_cleared() {
    // Clear all, set stops at 3 and 7, then clear the one at 3.
    let (_, t) = screen(12, 1, "\x1b[3g\x1b[1;4H\x1bH\x1b[1;8H\x1bH\r\tX");
    assert_eq!(t.term.cursor.x, 4);
    let (_, t) = screen(12, 1, "\x1b[3g\x1b[1;4H\x1bH\x1b[1;8H\x1bH\x1b[1;4H\x1b[g\r\t");
    assert_eq!(t.term.cursor.x, 7);
    // Without stops a tab goes to the last column.
    let (_, t) = screen(12, 1, "\x1b[3g\r\t");
    assert_eq!(t.term.cursor.x, 11);
}

#[test]
fn character_sets_and_shifts() {
    let (s, _) = screen(40, 1, "\x1b(0`abcdefghijklmnopqrstuvwxyz{|}~\x1b(B");
    assert_eq!(s[0].trim_end(), "◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·");
    // G2 via single shift (SS2), G3 via locking shift (LS3).
    let (s, _) = screen(10, 1, "\x1b*0\x1b+A\x1bNqq\x1bo#\x1bnx");
    assert_eq!(s[0].trim_end(), "─q£│");
}

#[test]
fn reports() {
    let (_, t) = screen(80, 24, "\x1b[c\x1b[5n\x1b[10;20H\x1b[6n\x1b[x");
    let r = String::from_utf8(t.term.replies.clone()).unwrap();
    assert_eq!(r, "\x1b[?62;22c\x1b[0n\x1b[10;20R\x1b[2;1;1;112;112;1;0x");
}

#[test]
fn soft_reset_keeps_screen() {
    let (s, t) = screen(6, 2, "\x1b[?6h\x1b[?25l\x1b[4hab\x1b[!p");
    assert_eq!(s[0].trim_end(), "ab");
    assert!(!t.term.modes.origin && t.term.modes.show_cursor && !t.term.modes.insert);
}
