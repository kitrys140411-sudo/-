# lumen-ui

The GUI toolkit of Lumen's own apps. It draws on the GPU (EGL + OpenGL ES 3,
the renderer `lumen-term` uses too), runs a Wayland window with client-side
decorations that match the desktop, and gives apps an immediate-mode API:
the app describes its window in one function, the toolkit does layout,
painting, input, keyboard focus and animation.

```
crates/lumen-ui/src/
  lib.rs          App, Options, the prelude, crate docs with a complete example app
  ui.rs           Ui: ids, widget state, animation, input queries, window requests
  layout.rs       the layout model (rows, columns, stacks; Fit/Px/Fill/Frac sizes)
  widgets/        every widget (see the table below)
  theme.rs        Theme: the desktop's theme file plus derived colors
  icons.rs        111 symbolic icons drawn from vector paths
  text.rs         fonts, cached text layouts (lumen-text: HarfBuzz shaping)
  image.rs        Pixels (decoded, Send) and Image (GPU texture handle)
  task.rs         Task and channel: background threads that wake the UI
  files.rs        directory listing, natural sort, sizes, dates, XDG places
  paint.rs        Painter: rectangles, shadows, lines, polylines, areas, arcs, text, images
  window.rs       the Wayland window runtime (sctk), damage-tracked rendering
  chrome.rs       client-side decorations: the header bar
  harness.rs      Harness: run a view in-process without a window (unit tests)
  testing.rs      LUMEN_UI_TEST event log and LUMEN_UI_TEST_MAP widget map
  gfx/            the GPU renderer (shared with lumen-term)
  bin/gallery.rs  lumen-ui-gallery: every widget, used by the UI tests
```

## An app in one screen

```rust
use lumen_ui::prelude::*;

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

fn main() {
    let options = Options::new("lumen-counter").title("Counter").size(420, 300);
    if let Err(e) = run(options, Counter::default()) {
        eprintln!("lumen-counter: {e}");
        std::process::exit(1);
    }
}
```

The crate docs (`src/lib.rs`) have a longer example: a task list with a
virtualized list, a header-bar button, a confirmation dialog, a toast and a
file loaded on a background thread.

## The model

- **`view` describes everything, every time.** `App::view(&mut self, ui)`
  runs whenever something may have changed: input, a running animation, a
  timer (`ui.request_repaint_after`), a background thread's wake-up. The app
  keeps its data in its own struct; widgets take `&mut` to the values they
  edit (`ui.switch("wifi", &mut self.wifi)`).
- **Responses come from the previous frame's layout.** A widget call returns
  a `Response` (`clicked`, `changed`, `submitted`, `hovered`, `focused`,
  `secondary_clicked`, drags…) computed against where the widget was drawn
  last frame, i.e. what the user saw when clicking. A frame whose view
  changed state is built again before it is painted, so the result of a
  click is on screen in the same frame.
- **Ids.** Widgets are identified by their name or label within the current
  scope (`ui.make_id`): two widgets with the same name in one scope get
  distinct ids by order. In loops over items whose order can change, wrap
  each item in `ui.push_id(key, |ui| …)` so its widgets keep their state
  (scroll position, cursor, animation) when others come and go. List and
  grid rows are scoped by index automatically.
- **Widget state.** Small private state (a text input's cursor and undo
  history, a scroll position, a menu being open) lives in the toolkit under
  the widget's id (`ui.state::<T>(id)`) and is dropped a while after the
  widget stops being shown. Everything the app cares about (selections,
  text, the open document) is app state.
- **Idle means idle.** After a frame, the toolkit compares the new draw list
  with the previous one and redraws only the damaged rectangle; a frame that
  changes nothing presents nothing, and without input, animation or timers
  no frame runs at all.

`App::close_requested` returns false to keep the window open (ask about
unsaved changes, then call `ui.close_window()`).

## Layout

A small flexbox (`layout.rs`). Every box has a width and a height `Size`:

| Size        | Meaning |
|-------------|---------|
| `Fit`       | as large as its content (the default) |
| `Px(n)`     | exactly `n` logical pixels |
| `Fill(w)`   | main axis: share `w` of the space left after the other children; cross axis: the parent's full inner size. `Size::FILL` is `Fill(1.0)` |
| `Frac(f)`   | fraction `f` of the parent's inner size |

Containers are `Frame::row()` (left to right), `Frame::column()` (top to
bottom) and `Frame::stack()` (on top of each other), with `padding`
(one value, `(vertical, horizontal)`, or `(top, right, bottom, left)` like
CSS), `gap`, `justify` (main axis: `Start`, `Center`, `End`,
`SpaceBetween`), `align` (cross axis: `Start`, `Center`, `End`, `Stretch`),
`min_width`/`max_width`/`min_height`/`max_height`, and decoration
(`background`, `radius`, `border`, `shadow`, `opacity`, `clip`). Every
widget with a box takes the same size builders through the `Sizing` trait:
`.width(Size::Px(200.0))`, `.fill_width()`, `.max_width(760.0)`.

```rust
Frame::row().fill_width().padding((10.0, 16.0)).gap(8.0).show(ui, |ui| {
    ui.search_input("find", &mut self.query);
    ui.spacer();                       // Fill(1) space: pushes the rest right
    ui.tool_button(Icon::Refresh, "Reload");
});
```

`ui.row` and `ui.column` are shorthands with the theme's grid gap (and
`ui.stack` for a plain stack);
`ui.space(px)`, `ui.spacer()` and `ui.separator()` fill rows and columns;
`ui.card(|ui| …)` is a padded rounded panel.

When the `Fit` children of a row do not fit, the ones that can shrink
(ellipsized or wrapped text, and boxes containing it) shrink first. Text
wraps at its final width (layout measures widths first, then heights).
Scroll areas and clipped boxes have a minimum width of their padding, so a
wide child scrolls or clips instead of widening the window.

The theme's spacing unit is 8 px (`theme.grid`); paddings and gaps are
multiples of 4 and 8. Body text is 14 px, controls are 32 px tall
(`CONTROL_HEIGHT`), list rows 32 to 56 px.

## Widgets

Most widgets have a shorthand on `Ui` and a builder for options:
`ui.button("Save")` or `Button::new("Save").icon(Icon::Save).primary().show(ui)`.

| Area | API |
|------|-----|
| Text | `ui.label`, `ui.heading`, `ui.title_text`, `ui.caption`, `ui.paragraph`, `ui.mono`; `Label::new(..).dim().wrap().max_lines(n).mono().align(..)`. Single-line labels ellipsize when there is no room |
| Buttons | `Button` (normal, `.primary()`, `.flat()`, `.destructive()`, `.round()`, `.toggled()`, `.enabled(false)`, `.tooltip(..)`), `Button::icon_only`, `ui.tool_button(icon, tooltip)` |
| Text entry | `TextInput` (placeholder, icon, clear button, password, mono; cursor, selection, word moves, clipboard and primary selection, undo/redo, context menu), `ui.search_input`; `TextEditor` + `TextDoc` for multi-line documents (virtualized lines, wrapping, line numbers, tabs, find highlights, undo) |
| Toggles | `ui.switch`, `ui.checkbox`, `ui.radio` |
| Values | `Slider` (`.step`), `ProgressBar` (fraction or `indeterminate()`), `Spinner` |
| Choices | `ui.dropdown`, `ui.tabs` (segmented), `ui.tab_bar` (closable document tabs: Ctrl+T/W, Ctrl+Page Up/Down), `ui.nav_list` / `ui.sidebar_layout` (sidebar navigation) |
| Menus | `ui.menu_button(icon, key, |m| …)`, `response.context_menu(ui, |m| …)`, `ui.open_popup_menu` + `ui.popup_menu` (menus for list items); `MenuItem` with icon, shortcut label, check mark, disabled, destructive; `m.heading`, `m.separator` |
| Overlays | `Dialog::new(key).title(..).show(ui, &mut open, |ui| …)`, `ui.confirm`, `ui.prompt`, `ui.dialog_buttons`; `ui.toast`, `ui.toast_with_action` + `ui.toast_action`; tooltips (`Button::tooltip`, `response.tooltip(ui, text)`); `ui.infobar(Severity, ..)` |
| Lists | `ListView::new(key, count, row_height)` and `ListView::grid(key, count, cell_w, cell_h)`: virtualized (only visible rows are built), multi-select (click, Ctrl, Shift, rubber band, Ctrl+A), keyboard navigation, activation (double click, Enter), context menus, `.reveal(i)`; `Selection` is app state. `ColumnHeaders` + `Column` + `SortOrder` make tables |
| Rows | `ui.group(title, |ui| …)` with `ListRow` (icon, title, subtitle, trailing widgets, activatable) and `ui.switch_row` — settings pages and device lists |
| Bars | `ui.path_bar` (breadcrumbs), `ui.shortcut_input` (records a key combination) |
| Scrolling | `ScrollArea::vertical() / horizontal() / both()`, `ui.scroll`: smooth wheel scrolling, touchpad scrolling with kinetic continuation, overlay scrollbars that can be dragged; `show_viewport` builds only what is visible |
| Graphics | `ui.icon`, `ui.icon_colored`, `ImageView` / `ui.image` (`Fit::Contain`, `Cover`, `Stretch`), `ui.canvas` / `Canvas` (custom drawing with `Painter`), `LineGraph` (live graphs) |
| Files | `FileChooser` (below), `files::read_dir`, `files::natural_cmp`, `files::format_size`, `files::format_time`, `files::places`, `files::icon_for` |

All widgets work with the keyboard: Tab and Shift+Tab move the focus (the
focus ring shows after keyboard use), Enter and Space activate, arrows move
within lists, menus, sliders and tabs, Escape closes menus and dialogs, the
Menu key opens context menus. Dialogs and menus keep the keyboard while open.

### Custom drawing

`ui.canvas(width, height, |p, rect| …)` paints with a `Painter`:
`fill`, `rounded`, `border`, `shadow`, `gradient`, `line`, `polyline`
(anti-aliased, round joins), `area` (a filled region under a polyline, for
graphs, fading toward the baseline), `arc` (rings and donut charts), `icon`,
`image`, `image_uv`, `text`, and clip/offset/opacity stacks. `Canvas::sense`
makes it interactive (`Sense::DRAG`, `Sense::SCROLL` with `ui.scroll_delta`
for zooming). The paint closure runs after layout, so it must be `'static`:
clone what it draws.

### Images

`Pixels` are decoded RGBA pixels, plain `Send` data made on worker threads:
`Pixels::load(path)` decodes PNG, JPEG, WebP, GIF, BMP and ICO and turns
photos upright by their EXIF orientation; `Pixels::load_thumbnail(path, max)`
and `pixels.thumbnail(w, h)` shrink; `rotated`, `flipped` transform.
`Image::from_pixels(p)` (on the UI thread) is the handle the view paints:
uploaded to a texture the first time it is drawn, freed with its last
handle; images larger than the GPU's limit are shrunk to fit.

## Theming

`Theme::load()` reads the desktop's theme file, the same one the compositor
uses: `$LUMEN_THEME`, then `~/.config/lumen/theme.toml`, then
`/usr/share/lumen/theme.toml`, else built-in defaults. Keys read:

```toml
[colors]      # accent, background, surface, surface_hover, border, text, text_dim, danger
accent = "#7aa2f7"
[fonts]       # ui, ui_size, monospace
ui = "Inter"
ui_size = 14
[geometry]    # grid, popover_radius
grid = 8
[animation]   # speed (0 turns animations off)
speed = 1.0
```

Unknown keys are ignored and malformed values keep their defaults. Window,
header, card, control, input, selection, focus ring and separator colors
are derived from the base colors (`Theme::derive`), so an accent change
recolors everything consistently. Apps read the theme with `ui.theme()`
(colors for custom drawing, `size_small()` / `size_heading()` /
`size_title()` for the type scale) and may replace it with `ui.set_theme`.
Translucent theme colors blend the way the compositor blends them (as if in
sRGB), while the renderer blends in linear light for gamma-correct text.

## Icons

`Icon` has 111 symbolic icons (files and folders, navigation, editing, media,
devices, Wi-Fi levels, Bluetooth, sound, power, system…), an original
line-style set drawn on a 24-unit grid with 2-unit strokes. They are
rasterized at the exact pixel size and tinted like text: `ui.icon(Icon::Folder,
16.0)`, `ui.icon_colored(icon, 20.0, ui.theme().accent)`, `Button::icon`,
`MenuItem::icon`, `ListRow::icon`. `Icon::ALL`, `icon.name()` and
`Icon::from_name("folder-open")` list and name them; the gallery's Icons
page shows them all. `files::icon_for(name, is_dir)` picks one by file type.

## Keyboard shortcuts

`ui.shortcut(Shortcut::ctrl('s'))` is true once when the shortcut was
pressed (and consumes it). Constructors: `Shortcut::ctrl`, `ctrl_shift`,
`alt`, `key` (no modifiers) and `new(mods, key)`. Rules that make shortcuts
behave the way users expect:

- A focused text input keeps its editing keys (Ctrl+A/C/X/V/Z, arrows,
  Delete, plain characters) wherever in `view` a shortcut for them is
  checked; other shortcuts (Ctrl+S, F5, Escape) still reach the app.
- While a menu or dialog is open, only shortcuts checked inside it fire;
  while a `shortcut_input` records, none do.
- Letter shortcuts work in any keyboard layout: `Key::Char` names the key's
  US-layout character when the layout types a non-Latin one, so Ctrl+S is
  Ctrl+S under the Russian layout too (while typing still gives Cyrillic).
- `MenuItem::shortcut` only shows the label; check the shortcut itself with
  `ui.shortcut` (usually at the top of `view`).

## Threads and background work

`view` runs on the UI thread and must stay quick (well under a millisecond
of app work). Anything slow (reading a directory, decoding an image, D-Bus
calls to iwd or BlueZ, running `wpctl`, polling `/proc`) runs on another
thread:

```rust
// One result: run a closure on a new thread.
if self.thumb.is_none() {
    let path = self.path.clone();
    self.thumb = Some(ui.spawn(move || Pixels::load_thumbnail(&path, 256)));
}
if let Some(Ok(p)) = self.thumb.as_mut().and_then(|t| t.take()) {
    self.image = Some(Image::from_pixels(p));
}

// A stream of values: a channel from a long-running thread.
let (tx, rx) = ui.channel();
std::thread::spawn(move || loop {
    std::thread::sleep(std::time::Duration::from_secs(1));
    if !tx.send(read_cpu_usage()) {
        break; // the window is gone
    }
});
self.samples = Some(rx);
// …and in view:
if let Some(rx) = &self.samples {
    for v in rx.drain() {
        self.cpu.push(v);
    }
}
```

`Sender::send` wakes the window, so `view` runs right after a result
arrives; nothing polls on a timer. `Task::take` returns the result once,
`Task::is_running` tells whether to show a spinner, `Receiver::drain`,
`latest` and `try_recv` read channels. `ui.waker()` gives a `Waker` for
other event sources. Values cross threads, so send `Pixels`, not `Image`.
For periodic work without a thread, `ui.request_repaint_after(duration)`
runs `view` again after a delay.

## The file chooser

A modal dialog inside the app's window; the app owns it while it is open
and shows it every frame until it returns a result:

```rust
if ui.button("Open…").clicked {
    self.chooser = Some(FileChooser::open("Open Image").filter("Images", &["*.png", "*.jpg"]));
}
if let Some(fc) = &mut self.chooser {
    if let Some(result) = fc.show(ui) {
        if let ChooserResult::Chosen(paths) = result {
            self.open(&paths[0]);
        }
        self.chooser = None;
    }
}
```

Modes: `FileChooser::open(title)`, `open_many(title)`, `save(title,
suggested_name)` and `folder(title)`; options: `.directory(path)` to start
in, `.filter(name, patterns)` (the first filter is active; an empty pattern
list matches everything), `.show_hidden(true)`. It has a places sidebar
(home, the XDG user folders, the file system), a path bar (Ctrl+L types a
location), back/forward/up, a name search, filters, hidden files (Ctrl+H),
sorting by name, size or date (column headers or the menu), type-ahead find,
a "New Folder" dialog (Ctrl+Shift+N) and, when saving, confirmation before
replacing a file. Escape cancels. The chooser logs `chooser folder <dir>`,
`chooser chosen <paths>` and `chooser cancelled` for tests.

## Performance

- No frames without changes: an idle window runs no `view`, no layout, no
  painting (the UI tests check that the frame counters stay put).
- Damage tracking: only the changed rectangle of the canvas is redrawn
  and presented.
- Text layouts are cached by their inputs and glyphs live in a texture
  atlas, so unchanged text costs no shaping and no uploads (the UI tests
  check that scrolling over known text uploads nothing).
- Lists, grids and the text editor build only what is visible.
- Animations are springs that stop when they settle; `ui.animate*` and
  `request_repaint` keep frames coming only while something moves.

- The window background is the canvas clear color, and the canvas is copied
  to the window buffer only where it changed since that buffer was last
  shown (EGL_EXT_buffer_age), so small updates stay small with a software
  rasterizer too.

With `LUMEN_UI_TEST` or `LUMEN_UI_TEST_MAP` set, the window prints `perf
frames=N avg_ms=… max_ms=… ui_ms=… uploads=…` after each burst of activity
(and every 120 frames of a long one): the time per presented frame from the
start of `view` to the end of `eglSwapBuffers` (with llvmpipe that includes
rasterizing), the part of it the toolkit spent building, laying out and
painting the draw list, and the glyph and image texture uploads meanwhile.

## Testing apps

### In process: `Harness`

`Harness` runs an app's view without a window, compositor or GPU (it needs
the fonts), with a simulated clock: click and type by label, then check the
app's state, the widgets and the event log.

```rust
let mut app = Counter::default();
let mut h = Harness::new(400.0, 300.0).unwrap();
h.frame(&mut app);
h.click("Add");
h.frame(&mut app);
assert_eq!(app.count, 1);
assert!(h.widget("1 clicks").is_some());
h.click("name");
h.type_text("Ada");
h.key(Key::Char('r'), Mods::CTRL);
h.settle(&mut app); // run frames until animations end
```

`widgets()` lists every visible labelled widget (`kind`, `label`, `rect`,
`value`, `focused`, `hovered`, `layer`): buttons, checkboxes, radios, menu
items, rows, tabs and sidebar entries by their text; inputs, switches,
sliders and dropdowns by their name; icon-only buttons by their icon's name
(`ui.tool_button(Icon::Refresh, "Reload")` is `"refresh"`); `click` refuses a widget that is
covered by a menu, dialog, toast or tooltip, like a real pointer would;
`log()` returns the event lines listed below. `src/widget_tests.rs` has
many examples.

### In the headless compositor

Apps run unchanged in Lumen's headless compositor; two environment
variables expose what a test needs:

- `LUMEN_UI_TEST=1` prints events to stdout, one per line: `ready WxH`,
  `click <label>`, `toggle <name> on|off`, `select <label>` (radio),
  `select <name> <option>` (dropdown), `input <name> <text>`, `submit
  <name>`, `edit <name> <line>:<col> <line text>` (text editor), `slider
  <name> <value>`, `focus <label>`, `selection <indices>`, `activate
  <index>`, `menu <item>`, `tab <name> <label>`, `tab-select|tab-close
  <name> <title>`, `tab-new <name>`, `nav <label>`, `row <title>`, `toast
  <text>`, `dialog cancel`, `chooser …`, `path <name> <dir>`, `sort
  <column> ascending|descending`, `shortcut <name> <label>|disabled`,
  `perf …` and whatever the app logs with `ui.log(line)`.
- `LUMEN_UI_TEST_MAP=<file>` writes, after every frame that changed
  something, a JSON map of the visible labelled widgets with their
  rectangles in window coordinates, layer, value, focus and hover, plus
  `frames` (presented) and `renders` (view runs) counters.

`tools/lib/headless.py` wraps both in `UiApp`: `click(label, kind)`,
`widget`, `find`, `wait_gone`, `scroll`, `hover`, `wait_log(regex)`,
`settle()` (widgets stopped moving) and `idle()` (no frames), with the
compositor's synthetic keyboard (`c.key("ctrl", "s")`, `c.type("text")`) and
screenshots (`c.screenshot(name)`). `tools/ctests/test_ui.py` drives the
gallery that way:

```sh
cargo build --target x86_64-unknown-linux-musl -p lumen-ui --bin lumen-ui-gallery --target-dir build/target
LUMEN_PROFILE=debug python3 tools/ctest.py ui.      # in the build container; results in build/ctest-results/ui.*/
```

## The gallery

`lumen-ui-gallery [--page NAME] [--size WxH]` shows every widget on eight
pages (Controls, Lists, Text, Editor, Graphs, Dialogs, Settings, Icons);
Ctrl+1…8 switch pages, Ctrl+O opens the file chooser. It is the reference
for how the widgets are meant to be combined.

## Sharing the renderer

`gfx` is the GPU layer both the toolkit and `lumen-term` use: EGL on the
Wayland surface, a persistent sRGB canvas (linear-light blending), a glyph
atlas with subpixel-positioned coverage masks, instanced quads, damage
regions. The toolkit submits an ordered `DrawList` (painter's order with
per-primitive clips); the terminal submits a layered `Scene`.
