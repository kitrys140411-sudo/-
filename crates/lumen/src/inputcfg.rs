//! Input settings (`input.toml`): keyboard layouts for xkb, and touchpad and
//! mouse settings for libinput devices.
//!
//! The shipped defaults are `rootfs/usr/share/lumen/input.toml`, compiled in.
//! The installed system file and then the user's file (`$LUMEN_INPUT`, else
//! `~/.config/lumen/input.toml`) are laid over them value by value, so a user
//! file only needs what it changes. A file with any error is left out as a
//! whole (the values before it stay in effect) and the error is reported.
//! `XKB_DEFAULT_LAYOUT`, `_VARIANT`, `_OPTIONS` and `_MODEL`, when set and not
//! empty, override the keyboard values (tests, quick experiments).

use serde::Deserialize;
use smithay::reexports::input as libinput;

/// The shipped defaults (also installed as /usr/share/lumen/input.toml).
pub const DEFAULTS: &str = include_str!("../../../rootfs/usr/share/lumen/input.toml");
const SYSTEM_FILE: &str = "/usr/share/lumen/input.toml";

/// A list written as an array or as a comma-separated string ("us,ru").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct List(pub Vec<String>);

impl<'de> Deserialize<'de> for List {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        let v = match Raw::deserialize(d).map_err(|_| serde::de::Error::custom("expected a list of strings or a comma-separated string"))? {
            Raw::One(s) if s.trim().is_empty() => Vec::new(),
            Raw::One(s) => s.split(',').map(|x| x.trim().to_string()).collect(),
            Raw::Many(v) => v.into_iter().map(|x| x.trim().to_string()).collect(),
        };
        Ok(List(v))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeyboardConfig {
    pub layouts: List,
    pub variants: List,
    pub options: List,
    pub model: String,
    pub labels: List,
    pub per_window: bool,
    pub repeat_delay: i32,
    pub repeat_rate: i32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        KeyboardConfig {
            layouts: List(vec!["us".into()]),
            variants: List::default(),
            options: List::default(),
            model: String::new(),
            labels: List::default(),
            per_window: false,
            repeat_delay: 400,
            repeat_rate: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClickMethod {
    Default,
    Clickfinger,
    Areas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccelProfile {
    Default,
    Adaptive,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TouchpadConfig {
    pub tap_to_click: bool,
    pub natural_scroll: bool,
    pub speed: f64,
    pub disable_while_typing: bool,
    pub click_method: ClickMethod,
    pub left_handed: bool,
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        TouchpadConfig {
            tap_to_click: true,
            natural_scroll: true,
            speed: 0.0,
            disable_while_typing: true,
            click_method: ClickMethod::Default,
            left_handed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseConfig {
    pub natural_scroll: bool,
    pub speed: f64,
    pub accel_profile: AccelProfile,
    pub left_handed: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        MouseConfig { natural_scroll: false, speed: 0.0, accel_profile: AccelProfile::Adaptive, left_handed: false }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    pub keyboard: KeyboardConfig,
    pub touchpad: TouchpadConfig,
    pub mouse: MouseConfig,
}

/// xkb layout names: letters, digits, '_' and '-' (a comma would split it).
fn valid_xkb_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl InputConfig {
    /// Check value ranges and names (types are checked while parsing).
    pub fn validate(&self) -> Result<(), String> {
        let k = &self.keyboard;
        if k.layouts.0.is_empty() {
            return Err("keyboard.layouts is empty".into());
        }
        if let Some(bad) = k.layouts.0.iter().find(|l| !valid_xkb_name(l)) {
            return Err(format!("keyboard.layouts: '{bad}' is not an xkb layout name (us, ru, de, ...)"));
        }
        if let Some(bad) = k.variants.0.iter().find(|v| !v.is_empty() && !valid_xkb_name(v)) {
            return Err(format!("keyboard.variants: '{bad}' is not an xkb variant name"));
        }
        if k.variants.0.len() > k.layouts.0.len() {
            return Err(format!("keyboard.variants has {} entries for {} layouts", k.variants.0.len(), k.layouts.0.len()));
        }
        if let Some(bad) = k.options.0.iter().find(|o| o.is_empty() || o.contains(char::is_whitespace)) {
            return Err(format!("keyboard.options: '{bad}' is not an xkb option (e.g. caps:escape)"));
        }
        if !(100..=5000).contains(&k.repeat_delay) {
            return Err(format!("keyboard.repeat_delay must be 100..5000 ms, not {}", k.repeat_delay));
        }
        if !(0..=100).contains(&k.repeat_rate) {
            return Err(format!("keyboard.repeat_rate must be 0..100 per second, not {}", k.repeat_rate));
        }
        for (name, v) in [("touchpad.speed", self.touchpad.speed), ("mouse.speed", self.mouse.speed)] {
            if !(-1.0..=1.0).contains(&v) {
                return Err(format!("{name} must be between -1 and 1, not {v}"));
            }
        }
        Ok(())
    }

    /// The compiled-in defaults.
    pub fn builtin() -> InputConfig {
        toml::from_str(DEFAULTS).expect("the built-in input settings are valid")
    }

    /// Path of the user's file.
    pub fn user_path() -> Option<std::path::PathBuf> {
        if let Some(p) = std::env::var_os("LUMEN_INPUT") {
            return Some(p.into());
        }
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config/lumen/input.toml"))
    }

    /// Defaults, system file and user file laid over each other, plus the
    /// xkb environment. Files with errors are left out; their errors are
    /// returned for reporting.
    pub fn load() -> (InputConfig, Vec<String>) {
        let mut files: Vec<(String, String)> = Vec::new();
        if let Ok(text) = std::fs::read_to_string(SYSTEM_FILE) {
            files.push((SYSTEM_FILE.into(), text));
        }
        let mut errors = Vec::new();
        if let Some(path) = InputConfig::user_path() {
            match std::fs::read_to_string(&path) {
                Ok(text) => files.push((path.display().to_string(), text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        let (mut cfg, file_errors) = layered(&files);
        errors.extend(file_errors);
        cfg.apply_xkb_env(|name| std::env::var(name).ok());
        (cfg, errors)
    }

    /// Re-read everything; any error makes the result `Err` (the caller
    /// keeps its current settings).
    pub fn reload() -> Result<InputConfig, String> {
        let (cfg, errors) = InputConfig::load();
        if errors.is_empty() { Ok(cfg) } else { Err(errors.join("\n")) }
    }

    /// `XKB_DEFAULT_*` variables replace the keyboard values when set.
    pub fn apply_xkb_env(&mut self, get: impl Fn(&str) -> Option<String>) {
        let get = |n: &str| get(n).filter(|v| !v.trim().is_empty());
        let split = |v: String| List(v.split(',').map(|x| x.trim().to_string()).collect());
        let k = &mut self.keyboard;
        if let Some(v) = get("XKB_DEFAULT_LAYOUT") {
            k.layouts = split(v);
            // Variants and labels belonged to the replaced layouts.
            k.variants = List::default();
            k.labels = List::default();
        }
        if let Some(v) = get("XKB_DEFAULT_VARIANT") {
            k.variants = split(v);
        }
        if let Some(v) = get("XKB_DEFAULT_OPTIONS") {
            k.options = split(v);
        }
        if let Some(v) = get("XKB_DEFAULT_MODEL") {
            k.model = v;
        }
    }
}

/// Lay `toml` tables over each other: tables merge key by key, other values replace.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// The built-in defaults with `files` (name, text) laid over them in order.
pub fn layered(files: &[(String, String)]) -> (InputConfig, Vec<String>) {
    let mut table: toml::Table = toml::from_str(DEFAULTS).expect("the built-in input settings are valid");
    let mut errors = Vec::new();
    for (name, text) in files {
        // Parsed alone first: errors then name the line in this file.
        let checked = toml::from_str::<InputConfig>(text).map_err(|e| e.to_string().trim().to_string()).and_then(|_| {
            let over: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
            let mut merged = table.clone();
            merge(&mut merged, over);
            let cfg: InputConfig = toml::Value::Table(merged.clone()).try_into().map_err(|e: toml::de::Error| e.to_string())?;
            cfg.validate()?;
            Ok(merged)
        });
        match checked {
            Ok(merged) => table = merged,
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    let cfg = toml::Value::Table(table).try_into().unwrap_or_else(|_| InputConfig::builtin());
    (cfg, errors)
}

/// Short panel label for an xkb layout: the language it types where that is
/// well known (us → EN, ua → UK), else the layout code itself.
pub fn layout_label(layout: &str) -> String {
    let code = layout.split(['(', ':']).next().unwrap_or(layout).trim().to_ascii_lowercase();
    let lang = match code.as_str() {
        "us" | "gb" | "au" | "nz" => "en",
        "ua" => "uk",
        "by" => "be",
        "kz" => "kk",
        "ge" => "ka",
        "am" => "hy",
        "il" => "he",
        "gr" => "el",
        "se" => "sv",
        "dk" => "da",
        "cz" => "cs",
        "ee" => "et",
        "jp" => "ja",
        "kr" => "ko",
        "cn" | "tw" => "zh",
        "vn" => "vi",
        "ir" => "fa",
        "rs" => "sr",
        "si" => "sl",
        "al" => "sq",
        "at" | "ch" => "de",
        "br" => "pt",
        "latam" => "es",
        other => other,
    };
    lang.chars().take(3).collect::<String>().to_uppercase()
}

impl KeyboardConfig {
    pub fn xkb_layout(&self) -> String {
        self.layouts.0.join(",")
    }

    /// One variant per layout ("" when none is given), or "" for all-default.
    pub fn xkb_variant(&self) -> String {
        if self.variants.0.iter().all(|v| v.is_empty()) {
            return String::new();
        }
        (0..self.layouts.0.len()).map(|i| self.variants.0.get(i).map(String::as_str).unwrap_or("")).collect::<Vec<_>>().join(",")
    }

    pub fn xkb_options(&self) -> Option<String> {
        (!self.options.0.is_empty()).then(|| self.options.0.join(","))
    }

    /// Panel labels, one per layout: configured, else derived; repeated
    /// labels get a number ("EN", "EN2").
    pub fn labels(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (i, layout) in self.layouts.0.iter().enumerate() {
            let base = self.labels.0.get(i).filter(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_else(|| layout_label(layout));
            let n = out.iter().filter(|l| l.trim_end_matches(|c: char| c.is_ascii_digit()) == base).count();
            out.push(if n == 0 { base } else { format!("{base}{}", n + 1) });
        }
        out
    }
}

/// What libinput reports a pointer device to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Touchpad,
    Mouse,
}

/// Settings for one device; `None` leaves libinput's default.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeviceSettings {
    pub tap: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub speed: Option<f64>,
    pub accel: Option<AccelProfile>,
    pub disable_while_typing: Option<bool>,
    pub click: Option<ClickMethod>,
    pub left_handed: Option<bool>,
}

impl InputConfig {
    pub fn device_settings(&self, kind: DeviceKind) -> DeviceSettings {
        match kind {
            DeviceKind::Touchpad => {
                let t = &self.touchpad;
                DeviceSettings {
                    tap: Some(t.tap_to_click),
                    natural_scroll: Some(t.natural_scroll),
                    speed: Some(t.speed),
                    accel: None,
                    disable_while_typing: Some(t.disable_while_typing),
                    click: (t.click_method != ClickMethod::Default).then_some(t.click_method),
                    left_handed: Some(t.left_handed),
                }
            }
            DeviceKind::Mouse => {
                let m = &self.mouse;
                DeviceSettings {
                    natural_scroll: Some(m.natural_scroll),
                    speed: Some(m.speed),
                    accel: (m.accel_profile != AccelProfile::Default).then_some(m.accel_profile),
                    left_handed: Some(m.left_handed),
                    ..DeviceSettings::default()
                }
            }
        }
    }
}

/// Touchpads are the pointer devices that can tap; other pointers are mice
/// (trackballs and trackpoints included).
pub fn device_kind(device: &libinput::Device) -> Option<DeviceKind> {
    if !device.has_capability(libinput::DeviceCapability::Pointer) {
        return None;
    }
    Some(if device.config_tap_finger_count() > 0 { DeviceKind::Touchpad } else { DeviceKind::Mouse })
}

/// Apply the settings for `device` (only what the device supports).
pub fn configure_device(device: &mut libinput::Device, cfg: &InputConfig) -> Option<DeviceKind> {
    let kind = device_kind(device)?;
    let s = cfg.device_settings(kind);
    if let Some(on) = s.tap.filter(|_| device.config_tap_finger_count() > 0) {
        let _ = device.config_tap_set_enabled(on);
    }
    if let Some(on) = s.natural_scroll.filter(|_| device.config_scroll_has_natural_scroll()) {
        let _ = device.config_scroll_set_natural_scroll_enabled(on);
    }
    if device.config_accel_is_available() {
        if let Some(speed) = s.speed {
            let _ = device.config_accel_set_speed(speed);
        }
        match s.accel {
            Some(AccelProfile::Flat) => {
                let _ = device.config_accel_set_profile(libinput::AccelProfile::Flat);
            }
            Some(AccelProfile::Adaptive) => {
                let _ = device.config_accel_set_profile(libinput::AccelProfile::Adaptive);
            }
            _ => {}
        }
    }
    if let Some(on) = s.disable_while_typing.filter(|_| device.config_dwt_is_available()) {
        let _ = device.config_dwt_set_enabled(on);
    }
    if let Some(method) = s.click {
        let m = if method == ClickMethod::Clickfinger { libinput::ClickMethod::Clickfinger } else { libinput::ClickMethod::ButtonAreas };
        if device.config_click_methods().contains(&m) {
            let _ = device.config_click_set_method(m);
        }
    } else if let Some(m) = device.config_click_default_method() {
        let _ = device.config_click_set_method(m);
    }
    if let Some(on) = s.left_handed.filter(|_| device.config_left_handed_is_available()) {
        let _ = device.config_left_handed_set(on);
    }
    Some(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, text: &str) -> (String, String) {
        (name.to_string(), text.to_string())
    }

    #[test]
    fn shipped_defaults() {
        let c = InputConfig::builtin();
        assert_eq!(c.keyboard.layouts.0, vec!["us", "ru"]);
        assert_eq!(c.keyboard.xkb_layout(), "us,ru");
        assert_eq!(c.keyboard.xkb_variant(), "");
        assert_eq!(c.keyboard.xkb_options(), None);
        assert_eq!(c.keyboard.labels(), vec!["EN", "RU"]);
        assert!(!c.keyboard.per_window);
        assert!(c.touchpad.tap_to_click && c.touchpad.natural_scroll && c.touchpad.disable_while_typing);
        assert!(!c.mouse.natural_scroll);
        assert_eq!(c.mouse.accel_profile, AccelProfile::Adaptive);
        c.validate().unwrap();
        assert_eq!(layered(&[]).0, c);
    }

    #[test]
    fn files_layer_value_by_value() {
        let system = file("system", "[keyboard]\nlayouts = \"us,de\"\n[mouse]\nspeed = 0.5\n");
        let user = file("user", "[keyboard]\nvariants = [\"\", \"nodeadkeys\"]\noptions = \"caps:escape, compose:ralt\"\n[touchpad]\nnatural_scroll = false\n");
        let (c, errors) = layered(&[system, user]);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.keyboard.xkb_layout(), "us,de");
        assert_eq!(c.keyboard.xkb_variant(), ",nodeadkeys");
        assert_eq!(c.keyboard.xkb_options().as_deref(), Some("caps:escape,compose:ralt"));
        assert_eq!(c.mouse.speed, 0.5);
        assert!(!c.touchpad.natural_scroll && c.touchpad.tap_to_click);
        assert_eq!(c.keyboard.repeat_delay, 400);
    }

    #[test]
    fn broken_files_are_left_out_whole() {
        let good = file("system", "[keyboard]\nlayouts = [\"us\", \"ua\"]\n");
        let typo = file("user", "[touchpad]\ntap_to_clik = false\n");
        let (c, errors) = layered(&[good.clone(), typo]);
        assert_eq!(c.keyboard.labels(), vec!["EN", "UK"]);
        assert!(c.touchpad.tap_to_click);
        assert!(errors.len() == 1 && errors[0].starts_with("user:") && errors[0].contains("tap_to_clik"), "{errors:?}");
        let range = file("user", "[keyboard]\nper_window = true\n[mouse]\nspeed = 2.0\n");
        let (c, errors) = layered(&[good.clone(), range]);
        assert!(!c.keyboard.per_window, "nothing of a rejected file applies");
        assert!(errors[0].contains("mouse.speed must be between -1 and 1"), "{errors:?}");
        for (text, msg) in [
            ("[keyboard]\nlayouts = []\n", "layouts is empty"),
            ("[keyboard]\nlayouts = [\"us ru\"]\n", "not an xkb layout name"),
            ("[keyboard]\nlayouts = \"us\"\nvariants = [\"a\", \"b\"]\n", "2 entries for 1 layouts"),
            ("[keyboard]\nrepeat_rate = 500\n", "repeat_rate"),
            ("[touchpad]\nclick_method = \"tap\"\n", "unknown variant"),
            ("[keyboard]\nlayouts = 5\n", "list of strings"),
            ("[keyboard\n", "TOML parse error"),
        ] {
            let (_, errors) = layered(&[file("f", text)]);
            assert!(errors.len() == 1 && errors[0].contains(msg), "{text:?}: {errors:?}");
        }
    }

    #[test]
    fn xkb_environment_overrides_the_keyboard() {
        let mut c = InputConfig::builtin();
        c.keyboard.labels = List(vec!["A".into(), "B".into()]);
        c.apply_xkb_env(|n| match n {
            "XKB_DEFAULT_LAYOUT" => Some("de,fr".into()),
            "XKB_DEFAULT_OPTIONS" => Some("".into()),
            _ => None,
        });
        assert_eq!(c.keyboard.xkb_layout(), "de,fr");
        assert_eq!(c.keyboard.labels(), vec!["DE", "FR"]);
        assert_eq!(c.keyboard.xkb_options(), None);
        let mut c = InputConfig::builtin();
        c.apply_xkb_env(|_| Some(" ".into()));
        assert_eq!(c, InputConfig::builtin(), "empty variables are unset");
    }

    #[test]
    fn labels() {
        assert_eq!(layout_label("us"), "EN");
        assert_eq!(layout_label("ru"), "RU");
        assert_eq!(layout_label("ua"), "UK");
        assert_eq!(layout_label("by"), "BE");
        assert_eq!(layout_label("de"), "DE");
        assert_eq!(layout_label("latam"), "ES");
        assert_eq!(layout_label("custom"), "CUS");
        let mut k = KeyboardConfig { layouts: List(vec!["us".into(), "gb".into(), "ru".into()]), ..KeyboardConfig::default() };
        assert_eq!(k.labels(), vec!["EN", "EN2", "RU"]);
        k.labels = List(vec!["".into(), "UK".into()]);
        assert_eq!(k.labels(), vec!["EN", "UK", "RU"]);
    }

    #[test]
    fn device_settings_follow_the_sections() {
        let mut c = InputConfig::builtin();
        let t = c.device_settings(DeviceKind::Touchpad);
        assert_eq!((t.tap, t.natural_scroll, t.disable_while_typing, t.click, t.accel), (Some(true), Some(true), Some(true), None, None));
        let m = c.device_settings(DeviceKind::Mouse);
        assert_eq!((m.tap, m.natural_scroll, m.accel, m.disable_while_typing), (None, Some(false), Some(AccelProfile::Adaptive), None));
        c.mouse.accel_profile = AccelProfile::Flat;
        c.touchpad.click_method = ClickMethod::Clickfinger;
        c.touchpad.speed = -0.25;
        assert_eq!(c.device_settings(DeviceKind::Mouse).accel, Some(AccelProfile::Flat));
        let t = c.device_settings(DeviceKind::Touchpad);
        assert_eq!((t.click, t.speed), (Some(ClickMethod::Clickfinger), Some(-0.25)));
    }
}
