//! The file chooser: a modal dialog inside the app's window to open a file
//! (or several), save a file or select a folder.
//!
//! The app owns the chooser while it is open and shows it every frame until
//! it returns a result:
//!
//! ```no_run
//! # use lumen_ui::{ChooserResult, FileChooser, Ui};
//! # struct App { chooser: Option<FileChooser> }
//! # impl App { fn view(&mut self, ui: &mut Ui) {
//! if ui.button("Open…").clicked {
//!     self.chooser = Some(FileChooser::open("Open Image").filter("Images", &["*.png", "*.jpg"]));
//! }
//! if let Some(fc) = &mut self.chooser {
//!     if let Some(result) = fc.show(ui) {
//!         if let ChooserResult::Chosen(paths) = result {
//!             println!("open {}", paths[0].display());
//!         }
//!         self.chooser = None;
//!     }
//! }
//! # } }
//! ```
//!
//! It has a places sidebar (home, the XDG user folders, the file system), a
//! path bar (Ctrl+L types a location), back/forward/up, a name search, file
//! type filters, hidden files (Ctrl+H), sorting by name, size or date,
//! type-ahead find, a "New Folder" dialog (Ctrl+Shift+N) and, when saving,
//! confirmation before replacing a file. Keyboard: arrows and Enter in the
//! list, Backspace or Alt+Up for the parent folder, Alt+Left/Right for
//! history, F5 to reload, Escape to cancel.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::files::{self, DirEntry, Place};
use crate::icons::Icon;
use crate::input::{Key, Shortcut};
use crate::layout::{Align, Size, Sizing};
use crate::text::TextAlign;
use crate::ui::Ui;

use super::list::{ListView, Selection};
use super::menu::MenuItem;
use super::text_input::{self, TextInput};
use super::{Button, Dialog, Frame, Label, Severity};

/// What the chooser is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserMode {
    /// One existing file.
    Open,
    /// One or more existing files.
    OpenMany,
    /// A file name to save to (existing files need confirmation).
    Save,
    /// A folder.
    Folder,
}

/// A named set of file name patterns ("Images": `*.png`, `*.jpg`).
#[derive(Clone, Debug, PartialEq)]
pub struct FileFilter {
    pub name: String,
    /// Shell patterns matched against the whole name, case-insensitively;
    /// empty matches everything.
    pub patterns: Vec<String>,
}

impl FileFilter {
    pub fn new(name: impl Into<String>, patterns: &[&str]) -> FileFilter {
        FileFilter { name: name.into(), patterns: patterns.iter().map(|p| p.to_string()).collect() }
    }

    pub fn matches(&self, file_name: &str) -> bool {
        self.patterns.is_empty() || self.patterns.iter().any(|p| files::glob_match(p, file_name))
    }
}

/// How a chooser ended.
#[derive(Clone, Debug, PartialEq)]
pub enum ChooserResult {
    /// The chosen paths (one, except in [`ChooserMode::OpenMany`]).
    Chosen(Vec<PathBuf>),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortKey {
    Name,
    Size,
    Modified,
}

/// A file chooser dialog (see the module docs).
pub struct FileChooser {
    mode: ChooserMode,
    title: String,
    dir: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    entries: Vec<DirEntry>,
    /// Indices into `entries` of what the list shows, in order.
    shown: Vec<usize>,
    loaded: Option<PathBuf>,
    load_error: Option<String>,
    loaded_at: SystemTime,
    stale: bool,
    filters: Vec<FileFilter>,
    filter: usize,
    search: String,
    shown_search: String,
    show_hidden: bool,
    sort: SortKey,
    descending: bool,
    selection: Selection,
    reveal: Option<usize>,
    /// Select this entry once the folder is loaded (going up selects the
    /// folder we came from; a new folder is selected).
    select_after_load: Option<String>,
    name: String,
    location: Option<String>,
    /// The location entry just opened: focus it.
    focus_location: bool,
    places: Vec<Place>,
    typeahead: (String, Option<Instant>),
    message: Option<String>,
    new_folder: (bool, String),
    replace: (bool, Option<PathBuf>),
    open: bool,
    frames: u32,
}

impl FileChooser {
    fn new(mode: ChooserMode, title: &str) -> FileChooser {
        FileChooser {
            mode,
            title: title.to_string(),
            dir: files::home(),
            back: Vec::new(),
            forward: Vec::new(),
            entries: Vec::new(),
            shown: Vec::new(),
            loaded: None,
            load_error: None,
            loaded_at: SystemTime::now(),
            stale: true,
            filters: Vec::new(),
            filter: 0,
            search: String::new(),
            shown_search: String::new(),
            show_hidden: false,
            sort: SortKey::Name,
            descending: false,
            selection: Selection::default(),
            reveal: None,
            select_after_load: None,
            name: String::new(),
            location: None,
            focus_location: false,
            places: files::places(),
            typeahead: (String::new(), None),
            message: None,
            new_folder: (false, String::new()),
            replace: (false, None),
            open: true,
            frames: 0,
        }
    }

    /// Choose one file to open.
    pub fn open(title: &str) -> FileChooser {
        FileChooser::new(ChooserMode::Open, title)
    }

    /// Choose one or more files to open.
    pub fn open_many(title: &str) -> FileChooser {
        FileChooser::new(ChooserMode::OpenMany, title)
    }

    /// Choose where to save, suggesting `name`.
    pub fn save(title: &str, name: &str) -> FileChooser {
        let mut c = FileChooser::new(ChooserMode::Save, title);
        c.name = name.to_string();
        c
    }

    /// Choose a folder.
    pub fn folder(title: &str) -> FileChooser {
        FileChooser::new(ChooserMode::Folder, title)
    }

    /// Start in `dir` (default: the home folder).
    pub fn directory(mut self, dir: impl Into<PathBuf>) -> FileChooser {
        self.dir = dir.into();
        self.loaded = None;
        self
    }

    /// Offer a file type filter; the first one added is active. Folders are
    /// always shown.
    pub fn filter(mut self, name: &str, patterns: &[&str]) -> FileChooser {
        self.filters.push(FileFilter::new(name, patterns));
        self.stale = true;
        self
    }

    /// Show dot files from the start.
    pub fn show_hidden(mut self, on: bool) -> FileChooser {
        self.show_hidden = on;
        self.stale = true;
        self
    }

    pub fn mode(&self) -> ChooserMode {
        self.mode
    }

    /// The folder shown.
    pub fn current_dir(&self) -> &Path {
        &self.dir
    }

    // -- Model ------------------------------------------------------------------------

    fn load(&mut self) {
        self.loaded = Some(self.dir.clone());
        self.loaded_at = SystemTime::now();
        match files::read_dir(&self.dir) {
            Ok(v) => {
                self.entries = v;
                self.load_error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.load_error = Some(match e.kind() {
                    std::io::ErrorKind::PermissionDenied => "You do not have permission to open this folder.".into(),
                    std::io::ErrorKind::NotFound => "This folder does not exist.".into(),
                    _ => format!("Cannot open this folder: {e}"),
                });
            }
        }
        self.stale = true;
    }

    /// Recompute what is shown (filters, search, order), keeping the
    /// selection on the same entries.
    fn refilter(&mut self) {
        let selected: Vec<PathBuf> = self.selection.selected().filter_map(|i| self.shown.get(i).map(|&e| self.entries[e].path.clone())).collect();
        let cursor = self.selection.cursor.and_then(|i| self.shown.get(i)).map(|&e| self.entries[e].path.clone());
        let search = self.search.to_lowercase();
        let filter = self.filters.get(self.filter);
        let mode = self.mode;
        let hidden = self.show_hidden;
        self.shown = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                (hidden || !e.is_hidden())
                    && (search.is_empty() || e.name.to_lowercase().contains(&search))
                    && (e.is_dir || (mode != ChooserMode::Folder && filter.is_none_or(|f| f.matches(&e.name))))
            })
            .collect();
        let entries = &self.entries;
        let (key, desc) = (self.sort, self.descending);
        self.shown.sort_by(|&a, &b| {
            let (a, b) = (&entries[a], &entries[b]);
            // Folders first, whatever the order.
            b.is_dir.cmp(&a.is_dir).then_with(|| {
                let o = match key {
                    SortKey::Name => files::natural_cmp(&a.name, &b.name),
                    SortKey::Size => a.size.cmp(&b.size).then_with(|| files::natural_cmp(&a.name, &b.name)),
                    SortKey::Modified => a.modified.cmp(&b.modified).then_with(|| files::natural_cmp(&a.name, &b.name)),
                };
                if desc { o.reverse() } else { o }
            })
        });
        let mut sel = Selection::default();
        for (i, &e) in self.shown.iter().enumerate() {
            let p = &self.entries[e].path;
            if selected.contains(p) {
                sel.toggle(i);
            }
            if cursor.as_ref() == Some(p) {
                sel.cursor = Some(i);
            }
        }
        self.selection = sel;
        if let Some(name) = self.select_after_load.take() {
            if let Some(i) = self.shown.iter().position(|&e| self.entries[e].name == name) {
                self.selection.select_only(i);
                self.reveal = Some(i);
            }
        }
        self.shown_search = self.search.clone();
        self.stale = false;
    }

    fn entry(&self, shown_index: usize) -> Option<&DirEntry> {
        self.shown.get(shown_index).map(|&e| &self.entries[e])
    }

    fn navigate(&mut self, ui: &mut Ui, to: PathBuf) {
        if to == self.dir {
            return;
        }
        self.back.push(std::mem::replace(&mut self.dir, to));
        self.forward.clear();
        self.entered(ui);
    }

    fn entered(&mut self, ui: &mut Ui) {
        self.loaded = None;
        self.selection = Selection::default();
        self.search.clear();
        self.message = None;
        self.location = None;
        ui.log(&format!("chooser folder {}", self.dir.display()));
    }

    fn go_up(&mut self, ui: &mut Ui) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            self.select_after_load = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
            self.navigate(ui, parent);
        }
    }

    fn go_back(&mut self, ui: &mut Ui) {
        if let Some(p) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.dir, p));
            self.entered(ui);
        }
    }

    fn go_forward(&mut self, ui: &mut Ui) {
        if let Some(p) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.dir, p));
            self.entered(ui);
        }
    }

    /// Typed paths: `~` is the home folder, relative ones start here.
    fn resolve(&self, typed: &str) -> PathBuf {
        let t = typed.trim();
        if t == "~" {
            files::home()
        } else if let Some(rest) = t.strip_prefix("~/") {
            files::home().join(rest)
        } else {
            self.dir.join(t)
        }
    }

    /// Enter or choose shown entry `i` (double click, Enter).
    fn activate(&mut self, ui: &mut Ui, i: usize) -> Option<ChooserResult> {
        let e = self.entry(i)?.clone();
        if e.is_dir {
            self.navigate(ui, e.path);
            return None;
        }
        match self.mode {
            ChooserMode::Open => Some(ChooserResult::Chosen(vec![e.path])),
            ChooserMode::OpenMany => {
                let mut paths = self.selected_files();
                if !paths.contains(&e.path) {
                    paths = vec![e.path];
                }
                Some(ChooserResult::Chosen(paths))
            }
            ChooserMode::Save => {
                self.name = e.name;
                self.try_save(ui)
            }
            ChooserMode::Folder => None,
        }
    }

    fn selected_files(&self) -> Vec<PathBuf> {
        self.selection.selected().filter_map(|i| self.entry(i)).filter(|e| !e.is_dir).map(|e| e.path.clone()).collect()
    }

    fn selected_dirs(&self) -> Vec<PathBuf> {
        self.selection.selected().filter_map(|i| self.entry(i)).filter(|e| e.is_dir).map(|e| e.path.clone()).collect()
    }

    fn can_accept(&self) -> bool {
        match self.mode {
            ChooserMode::Open | ChooserMode::OpenMany => !self.selected_files().is_empty() || self.selected_dirs().len() == 1,
            ChooserMode::Save => !self.name.trim().is_empty(),
            ChooserMode::Folder => self.load_error.is_none(),
        }
    }

    /// The accept button (Open, Save, Select).
    fn accept(&mut self, ui: &mut Ui) -> Option<ChooserResult> {
        match self.mode {
            ChooserMode::Open | ChooserMode::OpenMany => {
                let files = self.selected_files();
                if !files.is_empty() {
                    let files = if self.mode == ChooserMode::Open { files[..1].to_vec() } else { files };
                    return Some(ChooserResult::Chosen(files));
                }
                if let [dir] = self.selected_dirs().as_slice() {
                    self.navigate(ui, dir.clone());
                }
                None
            }
            ChooserMode::Save => self.try_save(ui),
            ChooserMode::Folder => {
                let dirs = self.selected_dirs();
                Some(ChooserResult::Chosen(vec![dirs.first().cloned().unwrap_or_else(|| self.dir.clone())]))
            }
        }
    }

    fn try_save(&mut self, ui: &mut Ui) -> Option<ChooserResult> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let path = self.resolve(&name);
        if path.is_dir() {
            // Typing a folder's name goes there.
            self.name.clear();
            self.navigate(ui, path);
            return None;
        }
        if !path.parent().is_some_and(Path::is_dir) {
            self.message = Some(format!("The folder of “{name}” does not exist."));
            return None;
        }
        if path.exists() {
            self.replace = (true, Some(path));
            return None;
        }
        Some(ChooserResult::Chosen(vec![path]))
    }

    fn create_folder(&mut self, ui: &mut Ui) {
        let name = self.new_folder.1.trim().to_string();
        if name.contains('/') {
            self.message = Some("Folder names cannot contain “/”.".into());
            return;
        }
        let path = self.dir.join(&name);
        match std::fs::create_dir(&path) {
            Ok(()) => {
                ui.log(&format!("chooser new-folder {}", path.display()));
                self.select_after_load = Some(name);
                self.loaded = None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.message = Some(format!("“{name}” already exists.")),
            Err(e) => self.message = Some(format!("Cannot create “{name}”: {e}")),
        }
    }

    // -- View ----------------------------------------------------------------------------

    /// Show the chooser (call every frame while it is open). Returns the
    /// result once, when a choice was made or the chooser was cancelled.
    pub fn show(&mut self, ui: &mut Ui) -> Option<ChooserResult> {
        if self.loaded.as_ref() != Some(&self.dir) {
            self.load();
        }
        if self.stale || self.search != self.shown_search {
            self.refilter();
        }
        let win = ui.window_size();
        let extra = if self.mode == ChooserMode::Save { 48.0 } else { 0.0 };
        let body_h = (win.y - 210.0 - extra).clamp(140.0, 470.0);
        let mut result = None;
        let mut open = self.open;
        let title = self.title.clone();
        ui.push_id("file-chooser", |ui| {
            Dialog::new("file-chooser").title(title).width(880.0).show(ui, &mut open, |ui| {
                result = self.body(ui, body_h);
            });
        });
        self.frames += 1;
        if !open && result.is_none() {
            result = Some(ChooserResult::Cancelled);
        }
        self.open = open && result.is_none();
        match &result {
            Some(ChooserResult::Chosen(p)) => ui.log(&format!("chooser chosen {}", p.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(";"))),
            Some(ChooserResult::Cancelled) => ui.log("chooser cancelled"),
            None => {}
        }
        result
    }

    fn body(&mut self, ui: &mut Ui, body_h: f32) -> Option<ChooserResult> {
        let mut result = None;
        let first = self.frames == 0;

        // Shortcuts of the whole dialog.
        if ui.shortcut(Shortcut::alt(Key::Up)) {
            self.go_up(ui);
        }
        if ui.shortcut(Shortcut::alt(Key::Left)) {
            self.go_back(ui);
        }
        if ui.shortcut(Shortcut::alt(Key::Right)) {
            self.go_forward(ui);
        }
        if ui.shortcut(Shortcut::ctrl('h')) {
            self.show_hidden = !self.show_hidden;
            self.stale = true;
        }
        if ui.shortcut(Shortcut::ctrl('l')) && self.location.is_none() {
            self.location = Some(format!("{}/", files::display_path(&self.dir).trim_end_matches('/')));
            self.focus_location = true;
        }
        if ui.shortcut(Shortcut::key(Key::F(5))) || ui.shortcut(Shortcut::ctrl('r')) {
            self.loaded = None;
        }
        if ui.shortcut(Shortcut::ctrl_shift('n')) {
            self.new_folder = (true, String::new());
        }
        if self.location.is_some() && ui.key_pressed(Key::Escape) {
            self.location = None;
        }

        // Save: the name comes first.
        if self.mode == ChooserMode::Save {
            Frame::row().fill_width().gap(12.0).show(ui, |ui| {
                Label::new("Name").show(ui);
                let r = TextInput::new("file name", &mut self.name).show(ui);
                if first {
                    // Focus the name with the part before the extension
                    // selected, ready to be typed over.
                    ui.request_focus(r.id);
                    let stem = match self.name.rfind('.') {
                        Some(i) if i > 0 => i,
                        _ => self.name.len(),
                    };
                    text_input::select_range(ui, r.id, 0, stem);
                }
                if r.submitted {
                    result = self.try_save(ui);
                }
            });
        }

        self.toolbar(ui);

        // Places | files.
        let mut list_id = None;
        Frame::row().fill_width().height(Size::Px(body_h)).show(ui, |ui| {
            Frame::column().width(Size::Px(184.0)).height(Size::FILL).gap(2.0).padding((0.0, 0.0, 10.0, 0.0)).show(ui, |ui| {
                let places = self.places.clone();
                for p in places {
                    if ui.nav_item(Some(p.icon), &p.name, p.path == self.dir).clicked {
                        self.navigate(ui, p.path.clone());
                    }
                }
            });
            ui.separator();
            Frame::column().fill_width().height(Size::FILL).padding((0.0, 0.0, 0.0, 0.0)).show(ui, |ui| {
                self.column_headers(ui);
                if let Some(err) = self.load_error.clone() {
                    empty_state(ui, Icon::Warning, &err);
                } else if self.shown.is_empty() {
                    let text = if !self.search.is_empty() { "No matching files" } else { "This folder is empty" };
                    empty_state(ui, Icon::FolderOpen, text);
                } else {
                    let r = self.list(ui);
                    list_id = Some(r.0);
                    if let Some(res) = r.1 {
                        result = Some(res);
                    }
                }
                if let Some(m) = self.message.clone() {
                    Frame::column().fill_width().padding((8.0, 8.0, 8.0, 0.0)).show(ui, |ui| {
                        if ui.infobar(Severity::Error, &m, Some("Dismiss")).clicked {
                            self.message = None;
                        }
                    });
                }
            });
        });
        if first && self.mode != ChooserMode::Save {
            if let Some(id) = list_id {
                ui.request_focus(id);
            }
        }

        // Footer: filter, cancel, accept.
        Frame::row().fill_width().gap(8.0).padding((4.0, 0.0, 0.0, 0.0)).show(ui, |ui| {
            if self.filters.len() > 1 {
                let names: Vec<&str> = self.filters.iter().map(|f| f.name.as_str()).collect();
                if ui.dropdown("file type", &mut self.filter, &names).changed {
                    self.stale = true;
                }
            } else if let Some(f) = self.filters.first() {
                Label::new(f.name.clone()).dim().show(ui);
            }
            ui.spacer();
            if ui.button("Cancel").clicked {
                result = Some(ChooserResult::Cancelled);
            }
            let label = match self.mode {
                ChooserMode::Open | ChooserMode::OpenMany => "Open",
                ChooserMode::Save => "Save",
                ChooserMode::Folder => "Select",
            };
            if Button::new(label).primary().enabled(self.can_accept()).min_width(88.0).show(ui).clicked {
                if let Some(r) = self.accept(ui) {
                    result = Some(r);
                }
            }
        });

        // Nested dialogs.
        if self.new_folder.0 {
            let (mut open, mut name) = (true, std::mem::take(&mut self.new_folder.1));
            if ui.prompt("new folder", &mut open, "New Folder", &mut name, "Create") == Some(true) {
                self.new_folder.1 = name;
                self.create_folder(ui);
                self.new_folder = (false, String::new());
            } else {
                self.new_folder = (open, name);
            }
        }
        if self.replace.0 {
            let name = self.replace.1.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let msg = "A file with this name already exists here. Replacing it overwrites its contents.";
            match ui.confirm("replace file", &mut self.replace.0, &format!("Replace “{name}”?"), msg, "Replace", true) {
                Some(true) => result = self.replace.1.take().map(|p| ChooserResult::Chosen(vec![p])),
                Some(false) => self.replace.1 = None,
                None => {}
            }
        }
        result
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        Frame::row().fill_width().gap(4.0).show(ui, |ui| {
            if Button::icon_only(Icon::Back).tooltip("Back (Alt+Left)").enabled(!self.back.is_empty()).show(ui).clicked {
                self.go_back(ui);
            }
            if Button::icon_only(Icon::Forward).tooltip("Forward (Alt+Right)").enabled(!self.forward.is_empty()).show(ui).clicked {
                self.go_forward(ui);
            }
            if Button::icon_only(Icon::Up).tooltip("Parent folder (Alt+Up)").enabled(self.dir.parent().is_some()).show(ui).clicked {
                self.go_up(ui);
            }
            ui.space(4.0);
            if let Some(mut loc) = self.location.take() {
                let r = TextInput::new("location", &mut loc).icon(Icon::FolderOpen).show(ui);
                if std::mem::take(&mut self.focus_location) {
                    ui.request_focus(r.id);
                    text_input::select_range(ui, r.id, loc.len(), loc.len());
                }
                self.location = Some(loc);
                if r.submitted {
                    let path = self.resolve(self.location.as_deref().unwrap_or(""));
                    if path.is_dir() {
                        self.navigate(ui, path);
                    } else if self.mode == ChooserMode::Save && path.parent().is_some_and(Path::is_dir) {
                        self.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let dir = path.parent().unwrap().to_path_buf();
                        self.navigate(ui, dir);
                    } else if matches!(self.mode, ChooserMode::Open | ChooserMode::OpenMany) && path.is_file() {
                        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                        self.select_after_load = path.file_name().map(|n| n.to_string_lossy().into_owned());
                        self.navigate(ui, dir);
                    } else {
                        self.message = Some(format!("“{}” does not exist.", path.display()));
                    }
                }
            } else {
                self.breadcrumbs(ui);
            }
            TextInput::new("search files", &mut self.search).icon(Icon::Search).placeholder("Search").clear_button().width(Size::Px(190.0)).min_width(120.0).show(ui);
            if Button::icon_only(Icon::FolderNew).tooltip("New Folder (Ctrl+Shift+N)").show(ui).clicked {
                self.new_folder = (true, String::new());
            }
            let (hidden, sort, desc) = (self.show_hidden, self.sort, self.descending);
            let mut action = None;
            ui.menu_button(Icon::MoreVertical, "chooser-menu", |m| {
                if m.add(MenuItem::new("Show Hidden Files").checked(hidden).shortcut(Shortcut::ctrl('h'))) {
                    action = Some(0);
                }
                m.separator();
                m.heading("Sort by");
                for (k, label) in [(SortKey::Name, "Name"), (SortKey::Size, "Size"), (SortKey::Modified, "Modified")] {
                    if m.add(MenuItem::new(label).checked(sort == k)) {
                        action = Some(1 + k as usize);
                    }
                }
                if m.add(MenuItem::new("Reverse Order").checked(desc)) {
                    action = Some(9);
                }
                m.separator();
                if m.add(MenuItem::new("Reload").icon(Icon::Refresh).shortcut(Shortcut::key(Key::F(5)))) {
                    action = Some(10);
                }
            });
            match action {
                Some(0) => self.show_hidden = !self.show_hidden,
                Some(1) => self.sort = SortKey::Name,
                Some(2) => self.sort = SortKey::Size,
                Some(3) => self.sort = SortKey::Modified,
                Some(9) => self.descending = !self.descending,
                Some(10) => self.loaded = None,
                _ => {}
            }
            if action.is_some() {
                self.stale = true;
            }
        });
    }

    /// The path as buttons, the last few levels of it.
    fn breadcrumbs(&mut self, ui: &mut Ui) {
        let home = files::home();
        let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
        let mut p = self.dir.as_path();
        loop {
            if p == home && home != Path::new("/") {
                crumbs.push(("Home".into(), p.to_path_buf()));
                break;
            }
            match p.file_name() {
                Some(n) => crumbs.push((n.to_string_lossy().into_owned(), p.to_path_buf())),
                None => {
                    crumbs.push(("Computer".into(), p.to_path_buf()));
                    break;
                }
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => break,
            }
        }
        crumbs.reverse();
        const MAX: usize = 4;
        let skipped = crumbs.len().saturating_sub(MAX);
        let mut go = None;
        Frame::row().fill_width().gap(0.0).clip().show(ui, |ui| {
            let t = ui.theme().clone();
            if skipped > 0 {
                let target = crumbs[skipped - 1].1.clone();
                if Button::icon_only(Icon::MoreHorizontal).tooltip(&files::display_path(&target)).id("crumbs-more").show(ui).clicked {
                    go = Some(target);
                }
            }
            let last = crumbs.len() - 1;
            for (i, (name, path)) in crumbs.iter().enumerate().skip(skipped) {
                if i > skipped || skipped > 0 {
                    ui.icon_colored(Icon::ChevronRight, 14.0, t.text_dim);
                }
                let b = Button::new(name.clone()).flat().id(("crumb", i)).toggled(i == last).max_width(220.0);
                if b.show(ui).clicked && i != last {
                    go = Some(path.clone());
                }
            }
        });
        if let Some(p) = go {
            self.navigate(ui, p);
        }
    }

    fn column_headers(&mut self, ui: &mut Ui) {
        let t = ui.theme().clone();
        Frame::row().fill_width().height(Size::Px(28.0)).gap(12.0).padding((0.0, 18.0, 0.0, 18.0)).show(ui, |ui| {
            ui.space(16.0);
            for (key, label, width, align) in [
                (SortKey::Name, "Name", Size::FILL, TextAlign::Start),
                (SortKey::Size, "Size", Size::Px(80.0), TextAlign::End),
                (SortKey::Modified, "Modified", Size::Px(116.0), TextAlign::Start),
            ] {
                let active = self.sort == key;
                let (_, r) = Frame::row().id(("sort", label)).sense(crate::Sense::CLICK).width(width).height(Size::FILL).gap(4.0).show_with_response(ui, |ui| {
                    if align == TextAlign::End {
                        ui.spacer();
                    }
                    let c = if active { t.text } else { t.text_dim };
                    Label::new(label).size(t.size_small()).weight(crate::Weight::Medium).color(c).show(ui);
                    if active {
                        ui.icon_colored(if self.descending { Icon::ChevronUp } else { Icon::ChevronDown }, 12.0, t.text_dim);
                    }
                });
                if r.clicked {
                    if active {
                        self.descending = !self.descending;
                    } else {
                        self.sort = key;
                        self.descending = false;
                    }
                    self.stale = true;
                }
            }
        });
        ui.separator();
    }

    /// The file list; returns its id and a result when an entry was chosen.
    fn list(&mut self, ui: &mut Ui) -> (crate::Id, Option<ChooserResult>) {
        let t = ui.theme().clone();
        let multi = self.mode == ChooserMode::OpenMany;
        let mut view = ListView::new("files", self.shown.len(), 36.0).reveal(self.reveal.take());
        if !multi {
            view = view.single_selection();
        }
        let now = self.loaded_at;
        let (entries, shown) = (&self.entries, &self.shown);
        let r = view.show(ui, &mut self.selection, |ui, item| {
            let e = &entries[shown[item.index]];
            let dim = e.is_hidden();
            let fg = if dim { t.text_dim } else { t.text };
            ui.icon_colored(e.icon(), 16.0, if e.is_dir { t.accent } else { t.text_dim });
            Label::new(e.name.clone()).color(fg).fill_width().show(ui);
            let size = if e.is_dir { String::new() } else { files::format_size(e.size) };
            Label::new(size).dim().align(TextAlign::End).width(Size::Px(80.0)).show(ui);
            let date = e.modified.map(|m| files::format_time(m, now)).unwrap_or_default();
            Label::new(date).dim().width(Size::Px(116.0)).show(ui);
        });
        let id = r.response.id;
        let mut result = None;
        // Save: picking a file takes its name.
        if r.changed && self.mode == ChooserMode::Save {
            if let Some(e) = self.selection.selected().next().and_then(|i| self.entry(i)).filter(|e| !e.is_dir) {
                self.name = e.name.clone();
            }
        }
        if let Some(i) = r.activated {
            result = self.activate(ui, i);
        }
        if r.response.focused && ui.keys_allowed() {
            if ui.key_pressed(Key::Backspace) {
                self.go_up(ui);
            }
            self.typeahead(ui);
        }
        (id, result)
    }

    /// Typing while the list has the keyboard jumps to the first entry
    /// starting with what was typed (the typed text is forgotten after a
    /// second without typing).
    fn typeahead(&mut self, ui: &mut Ui) {
        let now = ui.now();
        for k in ui.keys() {
            let Some(text) = k.text.clone().filter(|s| !s.trim().is_empty()) else { continue };
            if k.mods.command() {
                continue;
            }
            ui.take_key(k.key, k.mods);
            if self.typeahead.1.is_none_or(|t| now.duration_since(t) > Duration::from_secs(1)) {
                self.typeahead.0.clear();
            }
            self.typeahead.0.push_str(&text.to_lowercase());
            self.typeahead.1 = Some(now);
            let prefix = self.typeahead.0.clone();
            if let Some(i) = self.shown.iter().position(|&e| self.entries[e].name.to_lowercase().starts_with(&prefix)) {
                self.selection.select_only(i);
                self.reveal = Some(i);
                ui.request_repaint();
            }
        }
    }
}

/// An icon and a line of text centered in the remaining space.
fn empty_state(ui: &mut Ui, icon: Icon, text: &str) {
    let t = ui.theme().clone();
    Frame::column().fill_width().height(Size::FILL).align(Align::Center).justify(crate::Justify::Center).gap(12.0).show(ui, |ui| {
        ui.icon_colored(icon, 40.0, t.text_dim.with_alpha(0.6));
        Label::new(text).dim().show(ui);
    });
}
