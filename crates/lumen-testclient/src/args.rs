//! Hand-rolled command line parsing.

/// Window state requested right after the first configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialState {
    Maximized,
    Fullscreen,
}

/// Cursor shapes selectable with `--cursor-shape` (a subset of wp_cursor_shape_device_v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorName {
    Default,
    Pointer,
    Text,
    Grab,
    Grabbing,
    Move,
    Crosshair,
    Wait,
    NotAllowed,
    EwResize,
    NsResize,
}

impl CursorName {
    pub const ALL: [(&'static str, CursorName); 11] = [
        ("default", CursorName::Default),
        ("pointer", CursorName::Pointer),
        ("text", CursorName::Text),
        ("grab", CursorName::Grab),
        ("grabbing", CursorName::Grabbing),
        ("move", CursorName::Move),
        ("crosshair", CursorName::Crosshair),
        ("wait", CursorName::Wait),
        ("not-allowed", CursorName::NotAllowed),
        ("ew-resize", CursorName::EwResize),
        ("ns-resize", CursorName::NsResize),
    ];

    fn parse(s: &str) -> Option<CursorName> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == s)
            .map(|&(_, c)| c)
    }

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, c)| *c == self)
            .map_or("default", |&(name, _)| name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub title: String,
    pub app_id: String,
    /// Size in logical pixels used when the compositor leaves the choice to the client.
    pub size: (u32, u32),
    /// Fill color as (r, g, b).
    pub color: (u8, u8, u8),
    pub initial_state: Option<InitialState>,
    pub animate: bool,
    pub fractional: bool,
    pub cursor_shape: Option<CursorName>,
    pub copy: Option<String>,
    pub paste: bool,
    pub popup: bool,
    pub exit_after_ms: Option<u64>,
    pub close_on_key: bool,
    /// Paint colorful diagonal stripes over the body (for blur screenshots).
    pub stripes: bool,
    /// Be an X11 client (through Xwayland) instead of a Wayland one.
    pub x11: bool,
    /// X11: open a transient dialog once the main window is mapped.
    pub dialog: bool,
    /// Lock the pointer (zwp_locked_pointer_v1) on the main surface.
    pub lock_pointer: bool,
    /// Confine the pointer (zwp_confined_pointer_v1) to the main surface.
    pub confine_pointer: bool,
    /// Print zwp_relative_pointer_v1 motion.
    pub relative: bool,
    /// Keep the screen on (zwp_idle_inhibitor_v1 on the main surface).
    pub idle_inhibit: bool,
    /// Ask for every key (zwp_keyboard_shortcuts_inhibitor_v1).
    pub inhibit_shortcuts: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            title: "lumen-testclient".into(),
            app_id: "lumen.testclient".into(),
            size: (640, 400),
            color: (0x3b, 0x42, 0x61),
            initial_state: None,
            animate: false,
            fractional: false,
            cursor_shape: None,
            copy: None,
            paste: false,
            popup: false,
            exit_after_ms: None,
            close_on_key: false,
            stripes: false,
            x11: false,
            dialog: false,
            lock_pointer: false,
            confine_pointer: false,
            relative: false,
            idle_inhibit: false,
            inhibit_shortcuts: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Options),
    Help,
}

pub const USAGE: &str = "\
usage: lumen-testclient [OPTIONS]

  --title T              xdg_toplevel title (default \"lumen-testclient\")
  --app-id A             xdg_toplevel app_id (default \"lumen.testclient\")
  --size WxH             size when the compositor lets the client choose (default 640x400)
  --color #rrggbb        fill color (default #3b4261)
  --maximize             request maximized state after the first configure
  --fullscreen           request fullscreen state after the first configure
  --animate              redraw on every frame callback, print frame/presentation lines
  --fractional           use wp_fractional_scale_v1 + wp_viewporter
  --cursor-shape NAME    set a wp_cursor_shape on pointer enter (default, pointer, text,
                         grab, grabbing, move, crosshair, wait, not-allowed, ew-resize,
                         ns-resize)
  --copy TEXT            offer TEXT as the clipboard selection on keyboard focus
  --paste                print the clipboard selection on keyboard focus
  --popup                open an xdg_popup on right click, close it on the next click
  --exit-after-ms N      exit cleanly after N milliseconds
  --close-on-key         exit when Escape is pressed
  --stripes              paint colorful diagonal stripes over the body
  --lock-pointer         lock the pointer on the window (zwp_pointer_constraints_v1)
  --confine-pointer      confine the pointer to the window
  --relative             print relative pointer motion (zwp_relative_pointer_v1)
  --idle-inhibit         keep the screen on while visible (zwp_idle_inhibit_manager_v1)
  --inhibit-shortcuts    receive every key (zwp_keyboard_shortcuts_inhibit_manager_v1)
  --x11                  be an X11 client on $DISPLAY (Xwayland) instead; uses --title,
                         --app-id (WM_CLASS), --size, --color, --maximize, --fullscreen,
                         --popup (an override-redirect menu at the click), --exit-after-ms,
                         --close-on-key
  --dialog               with --x11: open a transient dialog once the window is mapped
  -h, --help             show this help

Events are printed to stdout, one line each, as space-separated key=value tokens.";

/// Parses the arguments (without the program name).
pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut opts = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // Accept both "--flag value" and "--flag=value".
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => args.next().ok_or_else(|| format!("{name} needs a value")),
            }
        };
        let takes_no_value = |name: &str| -> Result<(), String> {
            match inline {
                Some(_) => Err(format!("{name} does not take a value")),
                None => Ok(()),
            }
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--title" => opts.title = value("--title")?,
            "--app-id" => opts.app_id = value("--app-id")?,
            "--size" => opts.size = parse_size(&value("--size")?)?,
            "--color" => opts.color = parse_color(&value("--color")?)?,
            "--maximize" | "--fullscreen" => {
                takes_no_value(&flag)?;
                let state = if flag == "--maximize" {
                    InitialState::Maximized
                } else {
                    InitialState::Fullscreen
                };
                if opts.initial_state.is_some_and(|s| s != state) {
                    return Err("--maximize and --fullscreen are mutually exclusive".into());
                }
                opts.initial_state = Some(state);
            }
            "--animate" => {
                takes_no_value(&flag)?;
                opts.animate = true;
            }
            "--fractional" => {
                takes_no_value(&flag)?;
                opts.fractional = true;
            }
            "--cursor-shape" => {
                let name = value("--cursor-shape")?;
                let shape = CursorName::parse(&name)
                    .ok_or_else(|| format!("unknown cursor shape {name:?}"))?;
                opts.cursor_shape = Some(shape);
            }
            "--copy" => opts.copy = Some(value("--copy")?),
            "--paste" => {
                takes_no_value(&flag)?;
                opts.paste = true;
            }
            "--popup" => {
                takes_no_value(&flag)?;
                opts.popup = true;
            }
            "--exit-after-ms" => {
                let v = value("--exit-after-ms")?;
                let ms = v
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --exit-after-ms value {v:?}"))?;
                opts.exit_after_ms = Some(ms);
            }
            "--close-on-key" => {
                takes_no_value(&flag)?;
                opts.close_on_key = true;
            }
            "--stripes" => {
                takes_no_value(&flag)?;
                opts.stripes = true;
            }
            "--x11" | "--dialog" | "--lock-pointer" | "--confine-pointer" | "--relative" | "--idle-inhibit"
            | "--inhibit-shortcuts" => {
                takes_no_value(&flag)?;
                match flag.as_str() {
                    "--x11" => opts.x11 = true,
                    "--dialog" => opts.dialog = true,
                    "--lock-pointer" => opts.lock_pointer = true,
                    "--confine-pointer" => opts.confine_pointer = true,
                    "--relative" => opts.relative = true,
                    "--idle-inhibit" => opts.idle_inhibit = true,
                    _ => opts.inhibit_shortcuts = true,
                }
            }
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    if opts.lock_pointer && opts.confine_pointer {
        return Err("--lock-pointer and --confine-pointer are mutually exclusive".into());
    }
    if opts.dialog && !opts.x11 {
        return Err("--dialog needs --x11".into());
    }
    Ok(Command::Run(opts))
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let err = || format!("invalid size {s:?} (expected WxH, e.g. 640x400)");
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(err)?;
    let w = w.parse::<u32>().map_err(|_| err())?;
    let h = h.parse::<u32>().map_err(|_| err())?;
    if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) {
        return Err(format!("size {s:?} out of range (1..=16384)"));
    }
    Ok((w, h))
}

fn parse_color(s: &str) -> Result<(u8, u8, u8), String> {
    let err = || format!("invalid color {s:?} (expected #rrggbb)");
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err());
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
    Ok((channel(0)?, channel(2)?, channel(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Options, String> {
        match parse(args.iter().map(|s| s.to_string()))? {
            Command::Run(o) => Ok(o),
            Command::Help => Err("help".into()),
        }
    }

    #[test]
    fn defaults() {
        let o = run(&[]).unwrap();
        assert_eq!(o, Options::default());
        assert_eq!(o.size, (640, 400));
        assert_eq!(o.color, (0x3b, 0x42, 0x61));
        assert_eq!(o.title, "lumen-testclient");
        assert_eq!(o.app_id, "lumen.testclient");
    }

    #[test]
    fn all_flags() {
        let o = run(&[
            "--title",
            "hello world",
            "--app-id=org.example",
            "--size",
            "800x600",
            "--color",
            "#FF8000",
            "--fullscreen",
            "--animate",
            "--fractional",
            "--cursor-shape",
            "not-allowed",
            "--copy",
            "some text",
            "--paste",
            "--popup",
            "--exit-after-ms",
            "1500",
            "--close-on-key",
        ])
        .unwrap();
        assert_eq!(o.title, "hello world");
        assert_eq!(o.app_id, "org.example");
        assert_eq!(o.size, (800, 600));
        assert_eq!(o.color, (0xff, 0x80, 0x00));
        assert_eq!(o.initial_state, Some(InitialState::Fullscreen));
        assert!(o.animate && o.fractional && o.paste && o.popup && o.close_on_key);
        assert_eq!(o.cursor_shape, Some(CursorName::NotAllowed));
        assert_eq!(o.copy.as_deref(), Some("some text"));
        assert_eq!(o.exit_after_ms, Some(1500));
    }

    #[test]
    fn inline_values_and_color_without_hash() {
        let o = run(&["--size=10X20", "--color=0a0b0c", "--copy=", "--maximize"]).unwrap();
        assert_eq!(o.size, (10, 20));
        assert_eq!(o.color, (10, 11, 12));
        assert_eq!(o.copy.as_deref(), Some(""));
        assert_eq!(o.initial_state, Some(InitialState::Maximized));
    }

    #[test]
    fn errors() {
        assert!(run(&["--size", "0x10"]).is_err());
        assert!(run(&["--size", "10"]).is_err());
        assert!(run(&["--size", "axb"]).is_err());
        assert!(run(&["--color", "#12345"]).is_err());
        assert!(run(&["--color", "#12345g"]).is_err());
        assert!(run(&["--cursor-shape", "banana"]).is_err());
        assert!(run(&["--exit-after-ms", "-1"]).is_err());
        assert!(run(&["--title"]).is_err());
        assert!(run(&["--animate=1"]).is_err());
        assert!(run(&["--maximize", "--fullscreen"]).is_err());
        assert!(run(&["--bogus"]).is_err());
        assert!(run(&["positional"]).is_err());
    }

    #[test]
    fn gaming_and_x11_flags() {
        let o = run(&["--lock-pointer", "--relative", "--idle-inhibit", "--inhibit-shortcuts"]).unwrap();
        assert!(o.lock_pointer && o.relative && o.idle_inhibit && o.inhibit_shortcuts);
        assert!(!o.x11 && !o.confine_pointer);
        let o = run(&["--x11", "--dialog", "--popup"]).unwrap();
        assert!(o.x11 && o.dialog && o.popup);
        assert!(run(&["--lock-pointer", "--confine-pointer"]).is_err());
        assert!(run(&["--dialog"]).is_err());
        assert!(run(&["--x11=1"]).is_err());
    }

    #[test]
    fn help() {
        assert_eq!(parse(["--help".to_string()]), Ok(Command::Help));
        assert_eq!(parse(["-h".to_string()]), Ok(Command::Help));
    }

    #[test]
    fn every_cursor_name_parses() {
        for (name, shape) in CursorName::ALL {
            assert_eq!(
                run(&["--cursor-shape", name]).unwrap().cursor_shape,
                Some(shape)
            );
            assert_eq!(shape.name(), name);
        }
    }
}
