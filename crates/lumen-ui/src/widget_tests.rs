//! Behaviour tests of the larger widgets (file chooser, rows, graphs) and of
//! input routing, driven through the in-process [`Harness`].

use std::path::PathBuf;

use crate::geom::Vec2;
use crate::harness::Harness;
use crate::input::{Button, Key, Mods, Shortcut};
use crate::layout::{Size, Sizing};
use crate::{App, ChooserResult, FileChooser, Icon, LineGraph, Sense, Ui};

fn harness(w: f32, h: f32) -> Option<Harness> {
    let h = Harness::new(w, h).ok()?;
    if !h.has_font("Inter") {
        eprintln!("Inter not installed; skipping");
        return None;
    }
    Some(h)
}

/// An app from a closure.
struct F<S, V: FnMut(&mut S, &mut Ui)>(S, V);

impl<S, V: FnMut(&mut S, &mut Ui)> App for F<S, V> {
    fn view(&mut self, ui: &mut Ui) {
        (self.1)(&mut self.0, ui)
    }
}

/// A scratch directory: `sub/inner.txt`, `notes.txt`, `photo.png`,
/// `.hidden`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inner.txt"), "x").unwrap();
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();
    std::fs::write(dir.join("photo.png"), [0u8; 2048]).unwrap();
    std::fs::write(dir.join(".hidden"), "").unwrap();
    dir
}

struct Chooser {
    fc: Option<FileChooser>,
    result: Option<ChooserResult>,
}

fn chooser_app(fc: FileChooser) -> F<Chooser, impl FnMut(&mut Chooser, &mut Ui)> {
    F(Chooser { fc: Some(fc), result: None }, |s: &mut Chooser, ui: &mut Ui| {
        ui.label("Behind the chooser");
        if let Some(fc) = &mut s.fc {
            if let Some(r) = fc.show(ui) {
                s.result = Some(r);
                s.fc = None;
            }
        }
    })
}

fn labels(h: &Harness) -> Vec<String> {
    h.widgets().into_iter().map(|w| w.label).collect()
}

#[test]
fn chooser_filters_navigates_and_opens() {
    let Some(mut h) = harness(900.0, 640.0) else { return };
    let dir = scratch("open");
    let fc = FileChooser::open("Open Picture").directory(&dir).filter("Images", &["*.png"]).filter("All Files", &[]);
    let mut app = chooser_app(fc);
    h.settle(&mut app);
    let l = labels(&h);
    assert!(l.contains(&"photo.png".to_string()) && l.contains(&"sub".to_string()), "{l:?}");
    assert!(!l.contains(&"notes.txt".to_string()), "filtered out by the Images filter");
    assert!(!l.contains(&".hidden".to_string()), "hidden files are hidden");
    assert!(h.widgets().iter().any(|w| w.kind == "list" && w.focused), "the list has the keyboard");
    // The other filter shows everything; Ctrl+H shows dot files.
    h.click("file type");
    h.settle(&mut app);
    h.click("All Files");
    h.key(Key::Char('h'), Mods::CTRL);
    h.settle(&mut app);
    let l = labels(&h);
    assert!(l.contains(&"notes.txt".to_string()) && l.contains(&".hidden".to_string()), "{l:?}");
    // Into a folder and back up with the keyboard: the folder stays selected.
    h.double_click("sub");
    h.settle(&mut app);
    assert!(labels(&h).contains(&"inner.txt".to_string()));
    assert!(h.log().iter().any(|l| l.ends_with("/sub") && l.starts_with("chooser folder")));
    h.key(Key::Backspace, Mods::NONE);
    h.settle(&mut app);
    let list = h.widgets().into_iter().find(|w| w.kind == "list").unwrap();
    let sub_index = ["sub", ".hidden", "notes.txt", "photo.png"].iter().position(|&n| n == "sub").unwrap();
    assert_eq!(list.value.as_deref(), Some(sub_index.to_string().as_str()), "going up selects where we came from");
    // Type-ahead and Enter open the file.
    h.type_text("ph");
    h.frame(&mut app);
    h.key(Key::Enter, Mods::NONE);
    h.settle(&mut app);
    assert_eq!(app.0.result, Some(ChooserResult::Chosen(vec![dir.join("photo.png")])));
    assert!(h.widget("Open Picture").is_none(), "closed");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn chooser_saves_with_confirmation_and_creates_folders() {
    let Some(mut h) = harness(900.0, 640.0) else { return };
    let dir = scratch("save");
    let mut app = chooser_app(FileChooser::save("Save Notes", "Untitled.txt").directory(&dir));
    h.settle(&mut app);
    let name = h.widget("file name").unwrap();
    assert!(name.focused && name.value.as_deref() == Some("Untitled.txt"));
    // The stem is selected: typing replaces it and keeps the extension.
    h.type_text("notes");
    h.key(Key::Enter, Mods::NONE);
    h.settle(&mut app);
    assert!(h.widget("Replace “notes.txt”?").is_some(), "existing file: asks first");
    assert_eq!(app.0.result, None);
    h.click("Cancel");
    h.settle(&mut app);
    assert!(h.widget("Replace “notes.txt”?").is_none() && app.0.result.is_none());
    // A new folder, then save into it.
    h.click("folder-new");
    h.settle(&mut app);
    h.type_text("Projects");
    h.key(Key::Enter, Mods::NONE);
    h.settle(&mut app);
    assert!(dir.join("Projects").is_dir());
    assert!(labels(&h).contains(&"Projects".to_string()));
    h.double_click("Projects");
    h.settle(&mut app);
    h.click("Save");
    h.settle(&mut app);
    assert_eq!(app.0.result, Some(ChooserResult::Chosen(vec![dir.join("Projects/notes.txt")])));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn chooser_replace_and_cancel() {
    let Some(mut h) = harness(900.0, 640.0) else { return };
    let dir = scratch("replace");
    let mut app = chooser_app(FileChooser::save("Save", "notes.txt").directory(&dir));
    h.settle(&mut app);
    h.click("Save");
    h.settle(&mut app);
    h.click("Replace");
    h.settle(&mut app);
    assert_eq!(app.0.result, Some(ChooserResult::Chosen(vec![dir.join("notes.txt")])));
    // Escape cancels a chooser.
    let mut app = chooser_app(FileChooser::folder("Pick").directory(&dir));
    h.settle(&mut app);
    assert!(!labels(&h).contains(&"notes.txt".to_string()), "folder mode lists folders only");
    h.key(Key::Escape, Mods::NONE);
    h.settle(&mut app);
    assert_eq!(app.0.result, Some(ChooserResult::Cancelled));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn chooser_search_and_location() {
    let Some(mut h) = harness(900.0, 640.0) else { return };
    let dir = scratch("search");
    let mut app = chooser_app(FileChooser::open_many("Open").directory(&dir));
    h.settle(&mut app);
    h.click("search files");
    h.type_text("note");
    h.settle(&mut app);
    let l = labels(&h);
    assert!(l.contains(&"notes.txt".to_string()) && !l.contains(&"photo.png".to_string()), "{l:?}");
    // Ctrl+L: type a path.
    h.key(Key::Char('l'), Mods::CTRL);
    h.settle(&mut app);
    assert!(h.widget("location").unwrap().focused);
    h.type_text("sub");
    h.key(Key::Enter, Mods::NONE);
    h.settle(&mut app);
    assert!(labels(&h).contains(&"inner.txt".to_string()));
    // Several files at once.
    h.click("folder-new");
    h.settle(&mut app);
    h.key(Key::Escape, Mods::NONE);
    h.settle(&mut app);
    assert!(h.widget("New Folder").is_none() && h.widget("Open").is_some(), "Escape closes only the inner dialog");
    std::fs::write(dir.join("second.txt"), "2").unwrap();
    h.key(Key::Left, Mods::ALT);
    h.settle(&mut app);
    h.key(Key::F(5), Mods::NONE);
    h.settle(&mut app);
    h.click("notes.txt");
    h.frame(&mut app);
    let second = h.widget("second.txt").unwrap().rect.center();
    h.move_to(second);
    h.ui().input.mods = Mods::CTRL;
    h.click_at(second, Button::Left);
    h.frame(&mut app);
    h.ui().input.mods = Mods::NONE;
    h.click("Open");
    h.settle(&mut app);
    assert_eq!(app.0.result, Some(ChooserResult::Chosen(vec![dir.join("notes.txt"), dir.join("second.txt")])));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rows_switches_and_graphs() {
    let Some(mut h) = harness(640.0, 480.0) else { return };
    #[derive(Default)]
    struct S {
        wifi: bool,
        opened: u32,
        samples: Vec<f32>,
    }
    let mut app = F(S { samples: vec![10.0, 40.0, 25.0, 80.0], ..Default::default() }, |s: &mut S, ui: &mut Ui| {
        ui.group("Network", |ui| {
            ui.switch_row("Wi-Fi", Some("Connected"), &mut s.wifi);
            if crate::ListRow::new("Advanced").icon(Icon::Settings).activatable().show(ui, |_| {}).clicked {
                s.opened += 1;
            }
        });
        LineGraph::new("cpu").series(&s.samples).range(0.0, 100.0).height(Size::Px(80.0)).show(ui);
    });
    h.settle(&mut app);
    // A click on the row's title flips its switch.
    let title = h.widgets().into_iter().find(|w| w.kind == "row" && w.label == "Wi-Fi").unwrap();
    h.click_at(Vec2::new(title.rect.x + 40.0, title.rect.center().y), Button::Left);
    h.frame(&mut app);
    assert!(app.0.wifi);
    h.click("Wi-Fi");
    h.frame(&mut app);
    assert!(!app.0.wifi, "and the switch itself");
    h.click("Advanced");
    h.frame(&mut app);
    assert_eq!(app.0.opened, 1);
    assert_eq!(h.widget("cpu").unwrap().value.as_deref(), Some("80"));
    assert!(h.widget("Network").is_some(), "group heading");
    assert!(h.log().contains(&"toggle Wi-Fi on".to_string()));
}

#[test]
fn shortcuts_leave_editing_keys_to_a_focused_input() {
    let Some(mut h) = harness(640.0, 480.0) else { return };
    #[derive(Default)]
    struct S {
        text: String,
        select_all: u32,
        saved: u32,
    }
    let mut app = F(S::default(), |s: &mut S, ui: &mut Ui| {
        // Checked before the input is built.
        if ui.shortcut(Shortcut::ctrl('a')) {
            s.select_all += 1;
        }
        if ui.shortcut(Shortcut::ctrl('s')) {
            s.saved += 1;
        }
        ui.text_input("name", &mut s.text);
    });
    h.frame(&mut app);
    h.key(Key::Char('a'), Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(app.0.select_all, 1, "no input focused: the app gets Ctrl+A");
    h.click("name");
    h.frame(&mut app);
    h.type_text("abc");
    h.key(Key::Char('a'), Mods::CTRL);
    h.key(Key::Char('s'), Mods::CTRL);
    h.type_text("X");
    h.frame(&mut app);
    assert_eq!((app.0.select_all, app.0.saved, app.0.text.as_str()), (1, 1, "X"), "Ctrl+A selected the text, Ctrl+S saved");
}

#[test]
fn canvas_scroll_and_drag() {
    let Some(mut h) = harness(640.0, 480.0) else { return };
    #[derive(Default)]
    struct S {
        zoom: f32,
        pan: Vec2,
    }
    let mut app = F(S { zoom: 1.0, ..Default::default() }, |s: &mut S, ui: &mut Ui| {
        let r = crate::Canvas::new(Size::FILL, Size::FILL).sense("view", Sense { scroll: true, ..Sense::DRAG }).show(ui, |_, _| {});
        let d = ui.scroll_delta(r.id);
        s.zoom *= (1.0 - d.y / 500.0).clamp(0.5, 2.0);
        s.pan = s.pan + r.drag_delta;
    });
    h.frame(&mut app);
    h.frame(&mut app);
    let center = Vec2::new(320.0, 300.0);
    h.scroll_at(center, -100.0);
    h.frame(&mut app);
    assert!((app.0.zoom - 1.2).abs() < 1e-4, "{}", app.0.zoom);
    h.drag(center, center + Vec2::new(30.0, -10.0));
    h.frame(&mut app);
    assert_eq!(app.0.pan, Vec2::new(30.0, -10.0));
}

#[test]
#[should_panic(expected = "is covered by the menu")]
fn clicking_under_an_open_menu_is_a_test_error() {
    let Some(mut h) = harness(640.0, 480.0) else { panic!("is covered by the menu (no fonts)") };
    let mut app = F((), |_: &mut (), ui: &mut Ui| {
        ui.menu_button(Icon::Menu, "m", |m| {
            m.item("One");
            m.item("Two");
        });
        ui.button("Below");
    });
    h.frame(&mut app);
    h.click("menu");
    h.settle(&mut app);
    h.click("Below");
}

#[test]
fn background_tasks_reach_the_view() {
    let Some(mut h) = harness(640.0, 480.0) else { return };
    #[derive(Default)]
    struct S {
        task: Option<crate::Task<u64>>,
        result: Option<u64>,
    }
    let mut app = F(S::default(), |s: &mut S, ui: &mut Ui| {
        if s.task.is_none() && s.result.is_none() {
            s.task = Some(ui.spawn(|| (1..=10u64).product()));
        }
        if let Some(v) = s.task.as_mut().and_then(|t| t.take()) {
            s.result = Some(v);
            s.task = None;
        }
        match s.result {
            Some(v) => ui.label(format!("Result {v}")),
            None => ui.spinner(),
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.0.result.is_none() && std::time::Instant::now() < deadline {
        h.frame(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    h.frame(&mut app);
    assert!(h.widget("Result 3628800").is_some());
}

// -- Multi-line editor -------------------------------------------------------------------

use crate::{TextDoc, TextEditor};

fn editor_app(doc: TextDoc, wrap: bool) -> F<(TextDoc, u32), impl FnMut(&mut (TextDoc, u32), &mut Ui)> {
    F((doc, 0), move |s: &mut (TextDoc, u32), ui: &mut Ui| {
        // App shortcuts on keys the editor also uses: only Ctrl+S gets through.
        if ui.shortcut(Shortcut::ctrl('s')) {
            s.1 += 1;
        }
        if ui.shortcut(Shortcut::key(Key::Enter)) || ui.shortcut(Shortcut::key(Key::Up)) {
            s.1 += 100;
        }
        TextEditor::new("notes", &mut s.0).wrap(wrap).line_numbers(true).show(ui);
    })
}

fn type_keys(h: &mut Harness, keys: &[(Key, Mods)]) {
    for &(k, m) in keys {
        h.key(k, m);
    }
}

#[test]
fn editor_types_lines_keeps_indentation_and_moves_vertically() {
    let Some(mut h) = harness(640.0, 400.0) else { return };
    let mut app = editor_app(TextDoc::new(""), true);
    h.settle(&mut app);
    h.click("notes");
    h.frame(&mut app);
    h.type_text("fn main() {");
    h.key(Key::Enter, Mods::NONE);
    h.type_text("    let x = 1;");
    h.key(Key::Enter, Mods::NONE);
    h.type_text("y");
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "fn main() {\n    let x = 1;\n    y");
    assert_eq!(app.0 .1, 0, "Enter and Up stay with the focused editor");
    assert!(h.log().iter().any(|l| l == "edit notes 3:6     y"), "the log names the cursor's line");
    // Up keeps the column (monospace: the same x is the same column).
    h.key(Key::Up, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position(), (1, 5));
    h.key(Key::Up, Mods::NONE);
    h.key(Key::End, Mods::NONE);
    h.key(Key::Down, Mods::SHIFT);
    h.frame(&mut app);
    assert_eq!(app.0 .0.selected_text(), "\n    let x =", "Shift+Down from the end of a longer line keeps x");
    h.key(Key::Char('c'), Mods::CTRL);
    h.key(Key::Char('s'), Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(h.copied().as_deref(), Some("\n    let x ="));
    assert_eq!(app.0 .1, 1, "Ctrl+S reaches the app");
    // Home goes to the first non-blank, then to the line start.
    h.key(Key::Down, Mods::NONE);
    h.key(Key::Home, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position(), (2, 4));
    h.key(Key::Home, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position(), (2, 0));
    // Undo removes the typing word by word, Ctrl+Shift+Z redoes.
    h.key(Key::Char('z'), Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "fn main() {\n    let x = 1;\n    ");
    h.key(Key::Char('z'), Mods::CTRL_SHIFT);
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "fn main() {\n    let x = 1;\n    y");
}

#[test]
fn editor_tabs_indent_and_paste() {
    let Some(mut h) = harness(640.0, 400.0) else { return };
    let mut app = editor_app(TextDoc::new("one\ntwo"), true);
    h.settle(&mut app);
    h.click("notes");
    h.frame(&mut app);
    type_keys(&mut h, &[(Key::Home, Mods::CTRL), (Key::Tab, Mods::NONE)]);
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "\tone\ntwo");
    h.key(Key::Char('a'), Mods::CTRL);
    h.key(Key::Tab, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "\t\tone\n\ttwo", "Tab indents every selected line");
    h.key(Key::Tab, Mods::SHIFT);
    h.key(Key::Tab, Mods::SHIFT);
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "one\ntwo");
    h.key(Key::End, Mods::CTRL);
    h.key(Key::Char('v'), Mods::CTRL);
    h.frame(&mut app);
    h.paste("\r\nthree\nfour");
    h.frame(&mut app);
    assert_eq!(app.0 .0.text(), "one\ntwo\nthree\nfour", "pasted line ends are normalized");
    assert_eq!(app.0 .0.cursor_position(), (3, 4));
}

#[test]
fn editor_mouse_places_selects_words_and_drags() {
    let Some(mut h) = harness(640.0, 400.0) else { return };
    let mut app = editor_app(TextDoc::new("alpha beta gamma\nsecond line here\nthird"), true);
    h.settle(&mut app);
    let r = h.widget("notes").unwrap().rect;
    let lh = (14.0f32 * 1.5).round();
    // Far right of the second line: its end.
    let row = |k: f32| r.y + 10.0 + lh * (k + 0.5);
    h.click_at(Vec2::new(r.right() - 30.0, row(1.0)), Button::Left);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position(), (1, 16));
    // Double click a word (120 px in: past the gutter, inside "alpha" or
    // "beta" whatever the monospace font's width).
    h.double_click_at(Vec2::new(r.x + 120.0, row(0.0)));
    h.frame(&mut app);
    let word = app.0 .0.selected_text().to_string();
    assert!(["alpha", "beta"].contains(&word.as_str()), "a word is selected: {word:?}");
    // Drag from the start of line 0 to the middle of line 2.
    h.drag(Vec2::new(r.x + 46.0, row(0.0)), Vec2::new(r.right() - 10.0, row(2.0)));
    h.frame(&mut app);
    let sel = app.0 .0.selected_text().to_string();
    assert!(sel.ends_with("third") && sel.contains("second line here\n"), "{sel:?}");
    // Triple click selects the line.
    h.click_at(Vec2::new(r.x + 120.0, row(1.0)), Button::Left);
    h.click_at(Vec2::new(r.x + 120.0, row(1.0)), Button::Left);
    h.click_at(Vec2::new(r.x + 120.0, row(1.0)), Button::Left);
    h.frame(&mut app);
    assert_eq!(app.0 .0.selected_text(), "second line here\n");
}

#[test]
fn editor_wraps_long_lines_into_rows() {
    let Some(mut h) = harness(360.0, 400.0) else { return };
    let long = "word ".repeat(40);
    let mut app = editor_app(TextDoc::new(&format!("{long}\nshort")), true);
    h.settle(&mut app);
    h.click("notes");
    type_keys(&mut h, &[(Key::Home, Mods::CTRL), (Key::End, Mods::NONE)]);
    h.frame(&mut app);
    let (line, col) = app.0 .0.cursor_position();
    assert!(line == 0 && col > 10 && col < 60, "End goes to the end of the screen row: {col}");
    h.key(Key::Down, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position().0, 0, "Down moves to the next row of the same line");
    h.key(Key::End, Mods::CTRL);
    h.key(Key::Up, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position().0, 0, "Up from the next line lands on the long line's last row");
    assert!(app.0 .0.cursor_position().1 > 150);
}

#[test]
fn editor_shows_only_visible_lines_of_a_big_document() {
    let Some(mut h) = harness(640.0, 400.0) else { return };
    let text: String = (0..100_000).map(|i| format!("line {i} of a long file\n")).collect();
    let mut app = editor_app(TextDoc::new(&text), true);
    h.settle(&mut app);
    h.click("notes");
    h.frame(&mut app);
    let t0 = std::time::Instant::now();
    h.key(Key::End, Mods::CTRL);
    h.settle(&mut app);
    assert_eq!(app.0 .0.cursor_position(), (100_000, 0));
    // The last lines are on screen: clicking near the bottom lands there.
    let r = h.widget("notes").unwrap().rect;
    h.click_at(Vec2::new(r.x + 200.0, r.bottom() - 40.0), Button::Left);
    h.frame(&mut app);
    let line = app.0 .0.cursor_position().0;
    assert!(line > 99_980, "clicked line {line}");
    h.key(Key::Home, Mods::CTRL);
    h.settle(&mut app);
    h.click_at(Vec2::new(r.x + 200.0, r.y + 20.0), Button::Left);
    h.frame(&mut app);
    assert_eq!(app.0 .0.cursor_position().0, 0);
    eprintln!("100k lines: end, click, home, click in {:?}", t0.elapsed());
}

// -- Bars ----------------------------------------------------------------------------------

use crate::{Column, ColumnHeaders, SortOrder, Tab};

#[test]
fn path_bar_and_column_headers() {
    let Some(mut h) = harness(800.0, 400.0) else { return };
    let mut app = F((None::<PathBuf>, SortOrder::default(), 0u32), |s: &mut (Option<PathBuf>, SortOrder, u32), ui: &mut Ui| {
        if let Some(p) = ui.path_bar("files", std::path::Path::new("/usr/share/lumen/icons/apps")) {
            s.0 = Some(p);
        }
        let cols = [Column::new("Name", Size::FILL), Column::new("Size", Size::Px(80.0)).end()];
        if ColumnHeaders::new("table", &cols).show(ui, &mut s.1) {
            s.2 += 1;
        }
    });
    h.frame(&mut app);
    let l = labels(&h);
    // The last four levels, the rest behind "…".
    assert!(l.contains(&"apps".into()) && l.contains(&"share".into()) && !l.contains(&"usr".into()), "{l:?}");
    h.click("lumen");
    h.frame(&mut app);
    assert_eq!(app.0 .0.as_deref(), Some(std::path::Path::new("/usr/share/lumen")));
    h.click("more-horizontal");
    h.frame(&mut app);
    assert_eq!(app.0 .0.as_deref(), Some(std::path::Path::new("/usr")));
    h.click("Size");
    h.frame(&mut app);
    assert_eq!((app.0 .1, app.0 .2), (SortOrder { column: 1, descending: false }, 1));
    h.click("Size");
    h.frame(&mut app);
    assert_eq!(app.0 .1, SortOrder { column: 1, descending: true });
    assert_eq!(app.0 .1.apply(std::cmp::Ordering::Less), std::cmp::Ordering::Greater);
    assert!(h.log().contains(&"sort Size descending".to_string()));
}

#[test]
fn tab_bar_switches_closes_and_adds() {
    let Some(mut h) = harness(800.0, 400.0) else { return };
    struct S {
        docs: Vec<(String, bool)>,
        active: usize,
    }
    let docs = vec![("main.rs".to_string(), false), ("notes.md".to_string(), true), ("todo.txt".to_string(), false)];
    let mut app = F(S { docs, active: 0 }, |s: &mut S, ui: &mut Ui| {
        let tabs: Vec<Tab> = s.docs.iter().map(|(n, m)| Tab::new(n.clone()).icon(Icon::FileText).modified(*m)).collect();
        let r = ui.tab_bar("docs", &mut s.active, &tabs, true);
        if let Some(i) = r.close {
            s.docs.remove(i);
            s.active = s.active.min(s.docs.len().saturating_sub(1));
        }
        if r.new_tab {
            s.docs.push(("Untitled".into(), false));
            s.active = s.docs.len() - 1;
        }
    });
    h.frame(&mut app);
    h.click("notes.md");
    h.frame(&mut app);
    assert_eq!(app.0.active, 1);
    assert_eq!(h.widget("notes.md").unwrap().value.as_deref(), Some("on modified"));
    h.key(Key::PageDown, Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(app.0.active, 2);
    h.key(Key::Char('w'), Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(app.0.docs.len(), 2);
    h.click("plus");
    h.frame(&mut app);
    assert_eq!((app.0.docs.len(), app.0.active), (3, 2));
    // The close button of the first tab (shown while hovered).
    h.hover("main.rs");
    h.frame(&mut app);
    h.click("close main.rs");
    h.frame(&mut app);
    assert_eq!(app.0.docs.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), vec!["notes.md", "Untitled"]);
    let log = h.log();
    assert!(log.contains(&"tab-close docs main.rs".to_string()) && log.contains(&"tab-new docs".to_string()), "{log:?}");
}

#[test]
fn shortcut_field_records_a_combination() {
    let Some(mut h) = harness(600.0, 300.0) else { return };
    let mut app = F((Some(Shortcut::ctrl('t')), 0u32), |s: &mut (Option<Shortcut>, u32), ui: &mut Ui| {
        if ui.shortcut(Shortcut::ctrl('s')) {
            s.1 += 1;
        }
        ui.shortcut_input("terminal", &mut s.0);
    });
    h.frame(&mut app);
    assert_eq!(h.widget("terminal").unwrap().value.as_deref(), Some("Ctrl+T"));
    h.click("terminal");
    h.frame(&mut app);
    assert_eq!(h.widget("terminal").unwrap().value.as_deref(), Some("recording"));
    // Typing a plain letter is ignored; Ctrl+S is recorded, not an app shortcut.
    h.key(Key::Char('k'), Mods::NONE);
    h.key(Key::Char('s'), Mods::CTRL_SHIFT);
    h.frame(&mut app);
    assert_eq!(app.0 .0, Some(Shortcut::ctrl_shift('s')));
    assert_eq!(app.0 .1, 0);
    h.key(Key::Char('s'), Mods::CTRL);
    h.frame(&mut app);
    assert_eq!(app.0 .1, 1, "after recording, shortcuts work again");
    // Backspace disables, Escape cancels.
    h.key(Key::Enter, Mods::NONE);
    h.frame(&mut app);
    h.key(Key::Backspace, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0, None);
    assert_eq!(h.widget("terminal").unwrap().value.as_deref(), Some("disabled"));
    h.key(Key::Enter, Mods::NONE);
    h.key(Key::Escape, Mods::NONE);
    h.frame(&mut app);
    assert_eq!(app.0 .0, None);
    assert!(h.log().contains(&"shortcut terminal Ctrl+Shift+S".to_string()));
}

/// The README's first example app and its harness snippet, verbatim, so
/// the documentation stays true.
#[test]
fn readme_example_app() {
    use crate::prelude::*;

    #[derive(Default)]
    struct Counter {
        count: u32,
        name: String,
    }

    impl App for Counter {
        fn view(&mut self, ui: &mut Ui) {
            ui.title(format!("Counter — {}", self.count));
            if ui.shortcut(Shortcut::ctrl('r')) {
                self.count = 0;
            }
            Frame::column().fill_width().fill_height().padding(24.0).gap(12.0).show(ui, |ui| {
                ui.heading("Hello");
                ui.text_input("name", &mut self.name);
                Frame::row().gap(8.0).show(ui, |ui| {
                    if Button::new("Add").icon(Icon::Plus).primary().show(ui).clicked {
                        self.count += 1;
                    }
                    ui.label(format!("{} clicks", self.count));
                });
            });
        }
    }

    let Some(mut h) = harness(400.0, 300.0) else { return };
    let mut app = Counter::default();
    h.frame(&mut app);
    h.click("Add");
    h.frame(&mut app);
    assert_eq!(app.count, 1);
    assert!(h.widget("1 clicks").is_some());
    h.click("name");
    h.type_text("Ada");
    h.key(Key::Char('r'), Mods::CTRL);
    h.settle(&mut app);
    assert_eq!(app.name, "Ada");
    assert_eq!(app.count, 0, "Ctrl+R is not a text editing key, so it reaches the app");
}
