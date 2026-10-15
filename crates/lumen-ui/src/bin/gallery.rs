//! The lumen-ui gallery: every widget of the toolkit on a few pages, for
//! trying them out, for screenshots and for the headless UI tests
//! (`tools/ctests/test_ui.py` drives it through `LUMEN_UI_TEST` and the
//! widget map).
//!
//! `lumen-ui-gallery [--page NAME] [--size WxH]`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lumen_ui::prelude::*;

const PAGES: [(Icon, &str); 8] = [
    (Icon::Sliders, "Controls"),
    (Icon::List, "Lists"),
    (Icon::FileText, "Text"),
    (Icon::Edit, "Editor"),
    (Icon::Activity, "Graphs"),
    (Icon::Info, "Dialogs"),
    (Icon::Settings, "Settings"),
    (Icon::Grid, "Icons"),
];

struct Sample {
    cpu: f32,
    mem: f32,
}

#[derive(Default)]
struct Controls {
    name: String,
    search: String,
    password: String,
    notifications: bool,
    busy: bool,
    remember: bool,
    autosave: bool,
    quality: u8,
    volume: f32,
    size: usize,
    view: usize,
    bold: bool,
    clicks: u32,
}

/// An open document of the Editor page.
struct Doc {
    /// Identifies the document's editor state while tabs come and go.
    key: u64,
    name: String,
    doc: TextDoc,
}

struct Gallery {
    page: usize,
    c: Controls,
    // Lists
    list_sel: Selection,
    grid_sel: Selection,
    list_mode: usize,
    table_sel: Selection,
    table_sort: SortOrder,
    table_order: Vec<usize>,
    list_target: Option<usize>,
    deleted: Vec<usize>,
    // Graphs
    cpu: Vec<f32>,
    mem: Vec<f32>,
    samples: Option<Receiver<Sample>>,
    graphs_visible: Arc<AtomicBool>,
    paused: bool,
    // Dialogs
    confirm_open: bool,
    rename_open: bool,
    rename_text: String,
    chooser: Option<FileChooser>,
    last_choice: String,
    // Editor
    docs: Vec<Doc>,
    active_doc: usize,
    next_doc: u64,
    closing: Option<usize>,
    find: String,
    wrap: bool,
    numbers: bool,
    // Settings
    wifi: bool,
    bluetooth: bool,
    airplane: bool,
    output_volume: f32,
    brightness: f32,
    layout: usize,
    shortcuts: [Option<Shortcut>; 3],
    icon_sel: Selection,
}

impl Gallery {
    fn new(page: usize) -> Gallery {
        Gallery {
            page,
            c: Controls { notifications: true, remember: true, quality: 1, volume: 40.0, ..Default::default() },
            list_sel: Selection::default(),
            grid_sel: Selection::default(),
            list_mode: 0,
            table_sel: Selection::default(),
            table_sort: SortOrder::default(),
            table_order: Vec::new(),
            list_target: None,
            deleted: Vec::new(),
            cpu: Vec::new(),
            mem: Vec::new(),
            samples: None,
            graphs_visible: Arc::new(AtomicBool::new(false)),
            paused: false,
            confirm_open: false,
            rename_open: false,
            rename_text: String::new(),
            chooser: None,
            last_choice: "Nothing chosen yet".into(),
            docs: vec![
                Doc { key: 0, name: "main.rs".into(), doc: TextDoc::new(SAMPLE) },
                Doc { key: 1, name: "notes.md".into(), doc: TextDoc::new(NOTES) },
            ],
            active_doc: 0,
            next_doc: 2,
            closing: None,
            find: String::new(),
            wrap: true,
            numbers: true,
            wifi: true,
            bluetooth: false,
            airplane: false,
            output_volume: 65.0,
            brightness: 80.0,
            layout: 0,
            shortcuts: [Some(Shortcut::new(Mods { logo: true, ..Mods::NONE }, Key::Enter)), Some(Shortcut::key(Key::Other(0xff61))), None],
            icon_sel: Selection::default(),
        }
    }
}

impl App for Gallery {
    fn view(&mut self, ui: &mut Ui) {
        let page_name = PAGES[self.page].1;
        ui.title(format!("Gallery — {page_name}"));
        self.shortcuts(ui);
        let mut open_file = false;
        ui.header_end(|ui| {
            ui.menu_button(Icon::Menu, "main-menu", |m| {
                if m.add(MenuItem::new("Open File…").icon(Icon::Open).shortcut(Shortcut::ctrl('o'))) {
                    open_file = true;
                }
                if m.add(MenuItem::new("About Gallery").icon(Icon::Info)) {
                    m.ui.toast("lumen-ui gallery — every widget of the toolkit");
                }
                m.separator();
                if m.add(MenuItem::new("Quit").shortcut(Shortcut::ctrl('q'))) {
                    m.ui.close_window();
                }
            });
        });
        if open_file {
            self.chooser = Some(open_chooser());
        }
        // The content shows the page as of the start of this pass; a click
        // in the sidebar shows its page from the next pass on.
        let mut page = self.page;
        let shown = self.page;
        let items: Vec<(Icon, &str)> = PAGES.to_vec();
        ui.sidebar_layout(
            208.0,
            |ui| {
                if ui.nav_list("pages", &mut page, &items).changed {
                    ui.log(&format!("page {}", PAGES[page].1));
                }
            },
            |ui| {
                let name = PAGES[shown].1;
                self.graphs_visible.store(name == "Graphs", Ordering::Relaxed);
                match name {
                    "Controls" => self.controls(ui),
                    "Lists" => self.lists(ui),
                    "Text" => self.text(ui),
                    "Editor" => self.editor(ui),
                    "Graphs" => self.graphs(ui),
                    "Dialogs" => self.dialogs(ui),
                    "Settings" => self.settings(ui),
                    _ => self.icons(ui),
                }
            },
        );
        self.page = page;
        if let Some(fc) = &mut self.chooser {
            if let Some(r) = fc.show(ui) {
                self.last_choice = match r {
                    ChooserResult::Chosen(p) => p.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
                    ChooserResult::Cancelled => "Cancelled".into(),
                };
                self.chooser = None;
            }
        }
    }
}

/// A page: a scrolling column with a title and a short description.
fn page(ui: &mut Ui, title: &str, blurb: &str, f: impl FnOnce(&mut Ui)) {
    ScrollArea::vertical().id(("page", title)).padding((24.0, 32.0)).gap(16.0).show(ui, |ui| {
        Frame::column().max_width(760.0).gap(16.0).show(ui, |ui| {
            Frame::column().gap(4.0).show(ui, |ui| {
                ui.title_text(title);
                Label::new(blurb).dim().wrap().fill_width().show(ui);
            });
            f(ui);
        });
    });
}

/// A titled card of examples.
fn section(ui: &mut Ui, title: &str, f: impl FnOnce(&mut Ui)) {
    ui.card(|ui| {
        ui.heading(title);
        f(ui);
    });
}

impl Gallery {
    fn shortcuts(&mut self, ui: &mut Ui) {
        for (i, c) in ['1', '2', '3', '4', '5', '6', '7', '8'].into_iter().enumerate() {
            if ui.shortcut(Shortcut::ctrl(c)) {
                self.page = i;
                ui.log(&format!("page {}", PAGES[i].1));
            }
        }
        if ui.shortcut(Shortcut::ctrl('o')) {
            self.chooser = Some(open_chooser());
        }
        if ui.shortcut(Shortcut::ctrl('q')) {
            ui.close_window();
        }
    }

    fn controls(&mut self, ui: &mut Ui) {
        let c = &mut self.c;
        page(ui, "Controls", "Buttons, text inputs, toggles and value controls. Everything here works with the keyboard too: Tab moves between controls, Enter and Space activate them.", |ui| {
            section(ui, "Buttons", |ui| {
                Frame::row().gap(8.0).show(ui, |ui| {
                    if Button::new("Primary").primary().show(ui).clicked {
                        c.clicks += 1;
                    }
                    if ui.button("Normal").clicked {
                        c.clicks += 1;
                    }
                    if Button::new("Flat").flat().show(ui).clicked {
                        c.clicks += 1;
                    }
                    if Button::new("Delete").icon(Icon::Trash).destructive().show(ui).clicked {
                        c.clicks += 1;
                    }
                    Button::new("Disabled").enabled(false).show(ui);
                });
                Frame::row().gap(4.0).show(ui, |ui| {
                    ui.tool_button(Icon::Copy, "Copy");
                    ui.tool_button(Icon::Paste, "Paste");
                    ui.tool_button(Icon::Search, "Search");
                    if Button::icon_only(Icon::Edit).toggled(c.bold).tooltip("Toggle edit mode").show(ui).clicked {
                        c.bold = !c.bold;
                    }
                    ui.space(8.0);
                    Button::new("Round").icon(Icon::Plus).round().show(ui);
                    ui.spacer();
                    ui.caption(format!("Clicked {} times", c.clicks));
                });
            });
            section(ui, "Text Input", |ui| {
                Frame::row().gap(12.0).show(ui, |ui| {
                    Label::new("Name").width(Size::Px(84.0)).show(ui);
                    TextInput::new("name", &mut c.name).placeholder("Your name").show(ui);
                });
                Frame::row().gap(12.0).show(ui, |ui| {
                    Label::new("Search").width(Size::Px(84.0)).show(ui);
                    ui.search_input("search", &mut c.search);
                });
                Frame::row().gap(12.0).show(ui, |ui| {
                    Label::new("Password").width(Size::Px(84.0)).show(ui);
                    TextInput::new("password", &mut c.password).password().placeholder("At least 8 characters").show(ui);
                });
                if !c.name.is_empty() {
                    ui.caption(format!("Hello, {}!", c.name));
                }
            });
            section(ui, "Toggles", |ui| {
                Frame::row().gap(12.0).show(ui, |ui| {
                    ui.switch("notifications", &mut c.notifications);
                    ui.label("Notifications");
                    ui.spacer();
                    ui.switch("busy", &mut c.busy);
                    ui.label("Busy");
                });
                Frame::row().gap(24.0).show(ui, |ui| {
                    ui.checkbox("Remember me", &mut c.remember);
                    ui.checkbox("Autosave", &mut c.autosave);
                });
                Frame::row().gap(24.0).show(ui, |ui| {
                    ui.radio("Low", &mut c.quality, 0);
                    ui.radio("Medium", &mut c.quality, 1);
                    ui.radio("High", &mut c.quality, 2);
                });
            });
            section(ui, "Values", |ui| {
                Frame::row().gap(12.0).show(ui, |ui| {
                    ui.icon(Icon::VolumeLow, 18.0);
                    Slider::new("volume", &mut c.volume, 0.0..=100.0).step(1.0).show(ui);
                    ui.icon(Icon::Volume, 18.0);
                    Label::new(format!("{:.0}%", c.volume)).width(Size::Px(44.0)).align(TextAlign::End).show(ui);
                });
                ui.progress(c.volume / 100.0);
                if c.busy {
                    Frame::row().gap(12.0).show(ui, |ui| {
                        ui.spinner();
                        ProgressBar::indeterminate().show(ui);
                    });
                }
            });
            section(ui, "Choices", |ui| {
                Frame::row().gap(12.0).show(ui, |ui| {
                    ui.dropdown("size", &mut c.size, &["Small", "Medium", "Large", "Extra Large"]);
                    ui.spacer();
                    ui.tabs("view", &mut c.view, &["Day", "Week", "Month"]);
                });
            });
        });
    }

    fn lists(&mut self, ui: &mut Ui) {
        const COUNT: usize = 10_000;
        let t = ui.theme().clone();
        Frame::column().fill_width().height(Size::FILL).padding((20.0, 24.0)).gap(12.0).show(ui, |ui| {
            Frame::row().fill_width().gap(12.0).show(ui, |ui| {
                ui.title_text("Lists");
                ui.spacer();
                ui.tabs("list mode", &mut self.list_mode, &["List", "Grid"]);
            });
            let sel = if self.list_mode == 0 { &mut self.list_sel } else { &mut self.grid_sel };
            Label::new(format!(
                "{COUNT} items, only the visible ones are built. Click, Ctrl+click, Shift+click or drag on empty space to select; arrows, Page Up/Down and Ctrl+A work too. {} selected.",
                sel.count()
            ))
            .size(t.size_small())
            .dim()
            .wrap()
            .fill_width()
            .show(ui);
            let deleted = &self.deleted;
            let r = if self.list_mode == 0 {
                ListView::new("items", COUNT, 44.0).show(ui, sel, |ui, item| {
                    let gone = deleted.contains(&item.index);
                    ui.icon_colored(if item.index % 7 == 0 { Icon::Folder } else { Icon::FileText }, 18.0, if item.index % 7 == 0 { t.accent } else { t.text_dim });
                    Frame::column().fill_width().gap(0.0).show(ui, |ui| {
                        let name = format!("Item {}", item.index);
                        if gone {
                            Label::new(name).dim().show(ui);
                        } else {
                            Label::new(name).show(ui);
                        }
                        ui.caption(format!("{} kB · edited {} min ago", (item.index * 37) % 900 + 4, item.index % 60));
                    });
                })
            } else {
                ListView::grid("tiles", COUNT, 120.0, 112.0).show(ui, sel, |ui, item| {
                    ui.icon_colored(if item.index % 5 == 0 { Icon::Folder } else { Icon::Image }, 40.0, if item.index % 5 == 0 { t.accent } else { t.text_dim });
                    Label::new(format!("Tile {}", item.index)).align(TextAlign::Center).show(ui);
                })
            };
            if let Some(i) = r.activated {
                ui.toast(format!("Opened item {i}"));
            }
            if let Some((i, pos)) = r.context {
                self.list_target = i;
                ui.open_popup_menu("item-menu", pos);
            }
        });
        let target = self.list_target;
        let mut delete = false;
        ui.popup_menu("item-menu", |m| {
            if let Some(i) = target {
                m.heading(&format!("Item {i}"));
                if m.add(MenuItem::new("Open").icon(Icon::Open)) {
                    m.ui.toast(format!("Opened item {i}"));
                }
                if m.add(MenuItem::new("Copy Name").icon(Icon::Copy).shortcut(Shortcut::ctrl('c'))) {
                    m.ui.copy_text(format!("Item {i}"));
                }
                m.separator();
                if m.add(MenuItem::new("Delete").icon(Icon::Trash).destructive()) {
                    delete = true;
                }
            } else if m.add(MenuItem::new("Select All").shortcut(Shortcut::ctrl('a'))) {
                m.ui.log("select all");
            }
        });
        if delete {
            if let Some(i) = target {
                self.deleted.push(i);
                ui.toast_with_action(format!("Item {i} deleted"), Some("Undo"));
            }
        }
        if ui.toast_action("Undo") {
            self.deleted.pop();
        }
    }

    fn text(&mut self, ui: &mut Ui) {
        page(ui, "Text", "Inter for the interface, JetBrains Mono for code, shaped with HarfBuzz and drawn gamma-correct from a glyph atlas.", |ui| {
            section(ui, "Type Scale", |ui| {
                ui.title_text("Title — 28 px bold");
                ui.heading("Heading — 18 px semibold");
                ui.label("Body — 14 px regular, the size of nearly everything");
                ui.caption("Caption — 12 px, secondary");
                ui.mono("fn main() { println!(\"monospace\"); }");
            });
            section(ui, "Wrapping and Ellipsizing", |ui| {
                ui.paragraph("A paragraph wraps at spaces to the width of its box and grows as tall as it needs. Words longer than a line are split. Resize the window to watch it reflow; the layout is recomputed every frame and cached text layouts make that cheap.");
                Frame::row().gap(12.0).show(ui, |ui| {
                    Frame::column().width(Size::Px(220.0)).background(ui.theme().input_bg).radius(8.0).padding(10.0).show(ui, |ui| {
                        ui.label("A single line that is far too long for its box is shortened with an ellipsis");
                    });
                    Frame::column().width(Size::Px(220.0)).background(ui.theme().input_bg).radius(8.0).padding(10.0).show(ui, |ui| {
                        Label::new("Wrapped into at most two lines, and the second one is ellipsized when the text needs more room than that").max_lines(2).fill_width().show(ui);
                    });
                });
            });
            section(ui, "Scripts", |ui| {
                ui.label("Русский текст — съешь же ещё этих мягких французских булок");
                ui.label("Ελληνικά · Čeština · Türkçe · Tiếng Việt");
                ui.label("Emoji: 🎮 🚀 ✨ 👍🏽");
            });
        });
    }

    fn editor(&mut self, ui: &mut Ui) {
        let t = ui.theme().clone();
        // Ctrl+F finds, Enter and F3 go to the next match, Shift+F3 back.
        let mut focus_find = ui.shortcut(Shortcut::ctrl('f'));
        Frame::column().fill_width().height(Size::FILL).show(ui, |ui| {
            // Document tabs; closing one with changes asks first.
            let tabs: Vec<Tab> = self.docs.iter().map(|d| Tab::new(d.name.clone()).icon(Icon::FileText).modified(d.doc.is_modified())).collect();
            let r = ui.tab_bar("documents", &mut self.active_doc, &tabs, true);
            if let Some(i) = r.close {
                if self.docs[i].doc.is_modified() {
                    self.closing = Some(i);
                } else {
                    self.close_doc(i);
                }
            }
            if r.new_tab {
                self.docs.push(Doc { key: self.next_doc, name: format!("Untitled {}", self.next_doc), doc: TextDoc::default() });
                self.next_doc += 1;
                self.active_doc = self.docs.len() - 1;
            }
            let doc = &mut self.docs[self.active_doc];
            let matches = doc.doc.find_all(&self.find, false, 10_000);
            if ui.shortcut(Shortcut::key(Key::F(3))) {
                doc.doc.find_next(&self.find, false);
            }
            if ui.shortcut(Shortcut::new(Mods::SHIFT, Key::F(3))) {
                doc.doc.find_previous(&self.find, false);
            }
            Frame::row().fill_width().padding((10.0, 16.0)).gap(8.0).show(ui, |ui| {
                let r = TextInput::new("find", &mut self.find).icon(Icon::Search).placeholder("Find").clear_button().width(Size::Px(260.0)).show(ui);
                if std::mem::take(&mut focus_find) {
                    ui.request_focus(r.id);
                }
                if r.submitted {
                    doc.doc.find_next(&self.find, false);
                }
                if !self.find.is_empty() {
                    ui.caption(match matches.len() {
                        0 => "No matches".to_string(),
                        1 => "1 match".to_string(),
                        n => format!("{n} matches"),
                    });
                }
                if Button::icon_only(Icon::ChevronUp).tooltip("Previous Match").enabled(!matches.is_empty()).show(ui).clicked {
                    doc.doc.find_previous(&self.find, false);
                }
                if Button::icon_only(Icon::ChevronDown).tooltip("Next Match").enabled(!matches.is_empty()).show(ui).clicked {
                    doc.doc.find_next(&self.find, false);
                }
                ui.spacer();
                ui.switch("wrap", &mut self.wrap);
                ui.label("Wrap");
                ui.space(8.0);
                ui.switch("line numbers", &mut self.numbers);
                ui.label("Line Numbers");
            });
            ui.separator();
            // Each document keeps its own scroll position and cursor blink.
            ui.push_id(doc.key, |ui| {
                TextEditor::new("editor", &mut doc.doc).wrap(self.wrap).line_numbers(self.numbers).highlights(&matches).placeholder("Type something…").show(ui);
            });
            ui.separator();
            Frame::row().fill_width().padding((6.0, 16.0)).gap(16.0).show(ui, |ui| {
                let (l, c) = doc.doc.cursor_position();
                Label::new(format!("Ln {}, Col {}", l + 1, c + 1)).size(t.size_small()).dim().show(ui);
                Label::new(format!("{} lines", doc.doc.line_count())).size(t.size_small()).dim().show(ui);
                ui.spacer();
                if doc.doc.is_modified() {
                    Label::new("Modified").size(t.size_small()).color(t.accent).show(ui);
                }
            });
        });
        let mut open = self.closing.is_some();
        let name = self.closing.map(|i| self.docs[i].name.clone()).unwrap_or_default();
        let body = "Your changes will be lost if you close it without saving.";
        if let Some(yes) = ui.confirm("close-doc", &mut open, &format!("Close “{name}” without saving?"), body, "Close", true) {
            if let (true, Some(i)) = (yes, self.closing) {
                self.close_doc(i);
            }
            self.closing = None;
        }
    }

    /// Close document `i`; the last one is replaced by an empty document.
    fn close_doc(&mut self, i: usize) {
        self.docs.remove(i);
        if self.docs.is_empty() {
            self.docs.push(Doc { key: self.next_doc, name: format!("Untitled {}", self.next_doc), doc: TextDoc::default() });
            self.next_doc += 1;
        }
        self.active_doc = self.active_doc.min(self.docs.len() - 1);
    }

    fn graphs(&mut self, ui: &mut Ui) {
        // The sampler runs on its own thread and wakes the window for each
        // sample; it only sends while this page is shown, so the gallery
        // does no work elsewhere.
        if self.samples.is_none() {
            let (tx, rx) = ui.channel();
            let visible = self.graphs_visible.clone();
            std::thread::spawn(move || {
                let (mut cpu, mut mem, mut seed) = (30.0f32, 45.0f32, 0x2545_f491u32);
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    if !visible.load(Ordering::Relaxed) {
                        continue;
                    }
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let r = (seed % 1000) as f32 / 1000.0 - 0.5;
                    cpu = (cpu + r * 24.0).clamp(2.0, 98.0);
                    mem = (mem + r * 3.0).clamp(20.0, 90.0);
                    if !tx.send(Sample { cpu, mem }) {
                        break;
                    }
                }
            });
            self.samples = Some(rx);
        }
        if let Some(rx) = &self.samples {
            for s in rx.drain() {
                if !self.paused {
                    self.cpu.push(s.cpu);
                    self.mem.push(s.mem);
                }
            }
            let keep = 120;
            for v in [&mut self.cpu, &mut self.mem] {
                if v.len() > keep {
                    v.drain(..v.len() - keep);
                }
            }
        }
        let (cpu, mem) = (self.cpu.clone(), self.mem.clone());
        let paused = &mut self.paused;
        page(ui, "Graphs", "Live data from a background thread, which wakes the window for each sample, and custom drawing on a canvas.", |ui| {
            section(ui, "Processor and Memory", |ui| {
                Frame::row().gap(12.0).show(ui, |ui| {
                    let pal = graph_palette(ui.theme());
                    legend(ui, pal[0], &format!("Processor {:.0}%", cpu.last().copied().unwrap_or(0.0)));
                    legend(ui, pal[1], &format!("Memory {:.0}%", mem.last().copied().unwrap_or(0.0)));
                    ui.spacer();
                    let label = if *paused { "Resume" } else { "Pause" };
                    if Button::new(label).icon(if *paused { Icon::Play } else { Icon::Pause }).show(ui).clicked {
                        *paused = !*paused;
                    }
                });
                LineGraph::new("usage").series(&cpu).series(&mem).range(0.0, 100.0).capacity(120).height(Size::Px(160.0)).show(ui);
            });
            section(ui, "Canvas", |ui| {
                let pal = graph_palette(ui.theme());
                let dim = ui.theme().text_dim;
                ui.canvas(Size::FILL, Size::Px(150.0), move |p, r| {
                    // A donut chart from arcs.
                    let c = Vec2::new(r.x + 75.0, r.center().y);
                    let mut start = 0.0;
                    for (i, share) in [0.42f32, 0.27, 0.18, 0.13].iter().enumerate() {
                        let sweep = share * std::f32::consts::TAU;
                        p.arc(c, 56.0, 18.0, start + 0.03, sweep - 0.06, pal[i]);
                        start += sweep;
                    }
                    // A smooth curve over a filled area.
                    let x0 = r.x + 170.0;
                    let w = r.right() - x0 - 8.0;
                    let pts: Vec<Vec2> = (0..=64)
                        .map(|i| {
                            let f = i as f32 / 64.0;
                            Vec2::new(x0 + f * w, r.y + 75.0 - 50.0 * (f * 9.0).sin() * (1.0 - f * 0.6))
                        })
                        .collect();
                    p.area(&pts, r.bottom() - 8.0, pal[3].with_alpha(0.35), 0.1);
                    p.polyline(&pts, 2.5, pal[3]);
                    p.line(Vec2::new(x0, r.bottom() - 8.0), Vec2::new(x0 + w, r.bottom() - 8.0), 1.0, dim);
                });
            });
        });
    }

    fn dialogs(&mut self, ui: &mut Ui) {
        let mut open_chooser_kind = None;
        let last = self.last_choice.clone();
        let (confirm_open, rename_open, rename_text) = (&mut self.confirm_open, &mut self.rename_open, &mut self.rename_text);
        page(ui, "Dialogs", "Modal dialogs, the file chooser, menus, toasts and infobars. Dialogs keep the keyboard inside them; Escape cancels.", |ui| {
            section(ui, "Dialogs", |ui| {
                Frame::row().gap(8.0).show(ui, |ui| {
                    if ui.button("Delete File…").clicked {
                        *confirm_open = true;
                    }
                    if ui.button("Rename…").clicked {
                        *rename_text = "Holiday photos".into();
                        *rename_open = true;
                    }
                });
            });
            section(ui, "File Chooser", |ui| {
                Frame::row().gap(8.0).show(ui, |ui| {
                    if Button::new("Open File…").icon(Icon::Open).show(ui).clicked {
                        open_chooser_kind = Some(0);
                    }
                    if Button::new("Save As…").icon(Icon::Save).show(ui).clicked {
                        open_chooser_kind = Some(1);
                    }
                    if Button::new("Choose Folder…").icon(Icon::Folder).show(ui).clicked {
                        open_chooser_kind = Some(2);
                    }
                });
                Label::new(last).dim().wrap().fill_width().show(ui);
            });
            section(ui, "Menus and Toasts", |ui| {
                Frame::row().gap(8.0).show(ui, |ui| {
                    ui.menu_button(Icon::MoreVertical, "demo-menu", |m| {
                        m.heading("Edit");
                        m.add(MenuItem::new("Cut").icon(Icon::Cut).shortcut(Shortcut::ctrl('x')));
                        m.add(MenuItem::new("Copy").icon(Icon::Copy).shortcut(Shortcut::ctrl('c')));
                        m.add(MenuItem::new("Paste").icon(Icon::Paste).shortcut(Shortcut::ctrl('v')).enabled(false));
                        m.separator();
                        m.add(MenuItem::new("Show Hidden Files").checked(true));
                        m.add(MenuItem::new("Move to Trash").icon(Icon::Trash).destructive());
                    });
                    if ui.button("Show Toast").clicked {
                        ui.toast("Saved “report.odt”");
                    }
                    if ui.button("Toast with Action").clicked {
                        ui.toast_with_action("3 files moved to the trash", Some("Undo"));
                    }
                    if ui.toast_action("Undo") {
                        ui.toast("Restored 3 files");
                    }
                });
                let (_, r) = Frame::row().id("context-target").sense(Sense::CLICK).fill_width().height(Size::Px(56.0)).center().background(ui.theme().input_bg).radius(10.0).show_with_response(ui, |ui| {
                    ui.caption("Right-click here for a context menu");
                });
                r.context_menu(ui, |m| {
                    if m.item_icon(Icon::Refresh, "Reload") {
                        m.ui.toast("Reloaded");
                    }
                    m.item_icon(Icon::Info, "Properties");
                });
            });
            section(ui, "Infobars", |ui| {
                ui.infobar(Severity::Info, "A new version of this document is available.", Some("Reload"));
                ui.infobar(Severity::Success, "Backup finished: 1,284 files copied.", None);
                ui.infobar(Severity::Warning, "The disk is almost full (4.1 GB free).", Some("Clean Up"));
                ui.infobar(Severity::Error, "Could not connect to “Home Wi-Fi”: wrong password.", None);
            });
        });
        if let Some(r) = ui.confirm("delete", &mut self.confirm_open, "Delete “notes.txt”?", "The file will be deleted permanently. This cannot be undone.", "Delete", true) {
            ui.toast(if r { "Deleted “notes.txt”" } else { "Kept “notes.txt”" });
        }
        if let Some(true) = ui.prompt("rename", &mut self.rename_open, "Rename Folder", &mut self.rename_text, "Rename") {
            ui.toast(format!("Renamed to “{}”", self.rename_text));
        }
        match open_chooser_kind {
            Some(0) => self.chooser = Some(open_chooser()),
            Some(1) => self.chooser = Some(FileChooser::save("Save Document", "Untitled.txt").filter("Text Files", &["*.txt", "*.md"]).filter("All Files", &[])),
            Some(2) => self.chooser = Some(FileChooser::folder("Choose a Folder")),
            _ => {}
        }
    }

    fn settings(&mut self, ui: &mut Ui) {
        let t = ui.theme().clone();
        let (wifi, bluetooth, airplane) = (&mut self.wifi, &mut self.bluetooth, &mut self.airplane);
        let (volume, brightness, layout) = (&mut self.output_volume, &mut self.brightness, &mut self.layout);
        page(ui, "Settings", "The building blocks of settings pages: groups of rows with switches, values and actions.", |ui| {
            ui.group("Wireless", |ui| {
                ui.switch_row("Wi-Fi", Some(if *wifi { "Connected to Home" } else { "Off" }), wifi);
                ui.switch_row("Bluetooth", Some(if *bluetooth { "Discoverable as “lumen”" } else { "Off" }), bluetooth);
                ui.switch_row("Airplane Mode", Some("Turns off all wireless connections"), airplane);
            });
            if *wifi {
                ui.group("Networks", |ui| {
                    for (name, icon, secured, connected) in [
                        ("Home", Icon::Wifi, true, true),
                        ("Office 5G", Icon::WifiMedium, true, false),
                        ("Café Guest", Icon::WifiWeak, false, false),
                    ] {
                        let sub = match (connected, secured) {
                            (true, _) => "Connected",
                            (false, true) => "Secured",
                            (false, false) => "Open",
                        };
                        let r = ListRow::new(name).subtitle(sub).icon(icon).activatable().show(ui, |ui| {
                            if secured {
                                ui.icon_colored(Icon::Lock, 16.0, t.text_dim);
                            }
                            if connected {
                                ui.icon_colored(Icon::Check, 16.0, t.accent);
                            }
                        });
                        if r.clicked {
                            ui.toast(format!("Connecting to “{name}”…"));
                        }
                    }
                });
            }
            ui.group("Sound and Display", |ui| {
                ListRow::new("Output Volume").icon(Icon::Volume).show(ui, |ui| {
                    Slider::new("output volume", volume, 0.0..=100.0).step(1.0).width(Size::Px(220.0)).show(ui);
                });
                ListRow::new("Brightness").icon(Icon::Brightness).show(ui, |ui| {
                    Slider::new("brightness", brightness, 5.0..=100.0).step(1.0).width(Size::Px(220.0)).show(ui);
                });
                ListRow::new("Keyboard Layout").subtitle("Switch with Super+Space").icon(Icon::Keyboard).show(ui, |ui| {
                    ui.dropdown("keyboard layout", layout, &["English (US)", "Russian"]);
                });
            });
            ui.group("", |ui| {
                ListRow::new("About This Computer").icon(Icon::Info).activatable().show(ui, |_| {});
            });
        });
    }

    fn icons(&mut self, ui: &mut Ui) {
        let t = ui.theme().clone();
        Frame::column().fill_width().height(Size::FILL).padding((20.0, 24.0)).gap(12.0).show(ui, |ui| {
            ui.title_text("Icons");
            ui.caption(format!("{} symbolic icons, drawn from vector paths at the exact pixel size and tinted like text.", Icon::ALL.len()));
            ListView::grid("icon-grid", Icon::ALL.len(), 116.0, 88.0).show(ui, &mut self.icon_sel, |ui, item| {
                let icon = Icon::ALL[item.index];
                ui.icon_colored(icon, 24.0, if item.selected { t.accent } else { t.text });
                Label::new(icon.name()).size(t.size_small()).dim().align(TextAlign::Center).show(ui);
            });
        });
    }
}

fn legend(ui: &mut Ui, color: Color, text: &str) {
    Frame::row().gap(8.0).show(ui, |ui| {
        ui.canvas(Size::Px(10.0), Size::Px(10.0), move |p, r| p.rounded(r, 5.0, color));
        ui.label(text);
    });
}

/// The Editor page's starting text: code with tabs, and prose with long
/// lines to wrap.
const SAMPLE: &str = "// A small program: try typing, Tab, Shift+Tab and Ctrl+Z.
fn main() {
\tlet greeting = \"Hello, Lumen!\";
\tfor i in 0..3 {
\t\tprintln!(\"{greeting} #{i}\");
\t}
}

Lumen is a small desktop operating system. Its apps are built with lumen-ui, an immediate-mode toolkit \
drawn on the GPU: every frame the app describes its window, and the toolkit lays it out, paints only \
what changed and goes quiet when nothing moves.

Long lines wrap at the window edge; turn Wrap off to scroll sideways instead. Double-click selects a \
word, triple-click a line, and Ctrl+F finds text.
";

const NOTES: &str = "# Notes

- Ctrl+Page Up/Down switches documents, Ctrl+W closes one, Ctrl+T opens a new one.
- A dot on a tab marks unsaved changes; closing such a tab asks first.
";

fn open_chooser() -> FileChooser {
    FileChooser::open("Open File").filter("All Files", &[]).filter("Images", &["*.png", "*.jpg", "*.jpeg", "*.webp"]).filter("Text Files", &["*.txt", "*.md"])
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut page = 0;
    let mut size = (1040, 720);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--page" => {
                let want = args.next().unwrap_or_default().to_lowercase();
                page = PAGES.iter().position(|(_, n)| n.to_lowercase() == want).unwrap_or(0);
            }
            "--size" => {
                let v = args.next().unwrap_or_default();
                if let Some((w, h)) = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
                    size = (w, h);
                }
            }
            "--help" | "-h" => {
                println!("usage: lumen-ui-gallery [--page NAME] [--size WxH]");
                return;
            }
            other => {
                eprintln!("lumen-ui-gallery: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let options = Options::new("lumen-ui-gallery").title("Gallery").size(size.0, size.1).min_size(560, 400);
    if let Err(e) = run(options, Gallery::new(page)) {
        eprintln!("lumen-ui-gallery: {e}");
        std::process::exit(1);
    }
}
