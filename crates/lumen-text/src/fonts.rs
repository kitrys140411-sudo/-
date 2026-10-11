//! Font discovery and fallback chains.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::font::{Error, Font, FontFile, Library};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Weight {
    #[default]
    Regular,
    Medium,
    SemiBold,
    Bold,
}

impl Weight {
    fn style_names(self) -> &'static [&'static str] {
        match self {
            Weight::Regular => &["Regular", "Book", "Normal"],
            Weight::Medium => &["Medium", "Regular"],
            Weight::SemiBold => &["SemiBold", "Semi Bold", "DemiBold", "Bold"],
            Weight::Bold => &["Bold", "SemiBold"],
        }
    }
}

/// Directories searched for fonts ($LUMEN_FONT_DIR first, for tests).
fn font_dirs() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("LUMEN_FONT_DIR") {
        v.extend(d.split(':').map(String::from));
    }
    if let Ok(home) = std::env::var("HOME") {
        v.push(format!("{home}/.local/share/fonts"));
    }
    v.push("/usr/share/fonts".into());
    v
}

fn find_files(dir: &str, out: &mut Vec<String>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if path.is_dir() && depth < 4 {
            find_files(&path.to_string_lossy(), out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc") {
                out.push(path.to_string_lossy().into_owned());
            }
        }
    }
}

/// All installed font faces (scanned once per process; FreeType maps each
/// file and reads its headers).
pub struct FontDb {
    pub lib: Rc<Library>,
    pub faces: Vec<FontFile>,
}

impl FontDb {
    pub fn scan() -> Result<FontDb, Error> {
        let lib = Library::new()?;
        let mut files = Vec::new();
        for d in font_dirs() {
            find_files(&d, &mut files, 0);
        }
        let mut faces = Vec::new();
        for f in files {
            // Variable fonts duplicate the static instances (Inter ships both);
            // the static ones give exact weights without axis handling.
            if f.contains("Variable") {
                continue;
            }
            if let Ok(v) = FontFile::load_all(&lib, &f) {
                faces.extend(v);
            }
        }
        Ok(FontDb { lib, faces })
    }

    /// The face of `family` closest to `weight` (upright).
    pub fn find(&self, family: &str, weight: Weight) -> Option<FontFile> {
        let fam: Vec<&FontFile> = self
            .faces
            .iter()
            .filter(|f| f.family.eq_ignore_ascii_case(family) && !f.style.to_ascii_lowercase().contains("italic"))
            .collect();
        for name in weight.style_names() {
            if let Some(f) = fam.iter().find(|f| f.style.eq_ignore_ascii_case(name)) {
                return Some((*f).clone());
            }
        }
        fam.first().map(|f| (*f).clone())
    }

    /// The first color font (emoji).
    pub fn emoji(&self) -> Option<FontFile> {
        self.faces.iter().find(|f| f.color).cloned()
    }
}

/// A primary font with fallbacks; sized instances are created on demand.
pub struct FontSet {
    lib: Rc<Library>,
    files: Vec<FontFile>,
    instances: RefCell<HashMap<(usize, u32), Rc<Font>>>,
    /// Fallback choices per grapheme and size: selection opens fonts, which
    /// must not happen again for every frame of text.
    selected: RefCell<HashMap<(String, u32), usize>>,
}

impl FontSet {
    /// `families` in priority order (missing ones are skipped); color emoji and
    /// then every other installed font are appended as last-resort fallbacks.
    pub fn new(db: &FontDb, families: &[&str], weight: Weight) -> Result<FontSet, Error> {
        let mut files: Vec<FontFile> = families.iter().filter_map(|f| db.find(f, weight)).collect();
        if let Some(e) = db.emoji() {
            files.push(e);
        }
        for f in &db.faces {
            if !files.iter().any(|x| x.path == f.path && x.index == f.index) && !f.style.to_ascii_lowercase().contains("italic") {
                files.push(f.clone());
            }
        }
        if files.is_empty() {
            return Err(Error("no fonts installed".into()));
        }
        Ok(FontSet { lib: db.lib.clone(), files, instances: RefCell::new(HashMap::new()), selected: RefCell::new(HashMap::new()) })
    }

    pub fn primary(&self) -> &FontFile {
        &self.files[0]
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Font `index` of the chain at `px` pixels.
    pub fn font(&self, index: usize, px: f32) -> Option<Rc<Font>> {
        let key = (index, px.to_bits());
        if let Some(f) = self.instances.borrow().get(&key) {
            return Some(f.clone());
        }
        let font = Rc::new(self.files.get(index)?.instance(&self.lib, px).ok()?);
        let mut map = self.instances.borrow_mut();
        // Bound the cache (the UI uses a handful of sizes); keep the
        // primary fonts, which are needed by nearly every string.
        if map.len() > 512 {
            map.retain(|(i, _), _| *i < 2);
        }
        map.insert(key, font.clone());
        Some(font)
    }

    /// Index of the first font that has every character of `cluster` (a
    /// grapheme), ignoring joiners and variation selectors. Emoji presentation
    /// (VS16, or emoji-default characters) prefers a color font.
    pub fn select(&self, cluster: &str, px: f32) -> usize {
        // ASCII is in the primary font of any sane setup; skip the cache.
        if cluster.len() == 1 && self.font(0, px).is_some_and(|f| cluster.chars().all(|c| f.has_char(c))) {
            return 0;
        }
        let key = (cluster.to_string(), px.to_bits());
        if let Some(&i) = self.selected.borrow().get(&key) {
            return i;
        }
        let i = self.select_uncached(cluster, px);
        let mut map = self.selected.borrow_mut();
        if map.len() > 8192 {
            map.clear();
        }
        map.insert(key, i);
        i
    }

    fn select_uncached(&self, cluster: &str, px: f32) -> usize {
        let chars: Vec<char> = cluster.chars().filter(|c| !is_invisible_joiner(*c)).collect();
        if chars.is_empty() {
            return 0;
        }
        let wants_emoji = cluster.contains('\u{FE0F}') || chars.iter().any(|&c| is_emoji_presentation(c));
        let wants_text = cluster.contains('\u{FE0E}');
        let order: Vec<usize> = if wants_emoji && !wants_text {
            let mut v: Vec<usize> = (0..self.files.len()).filter(|&i| self.files[i].color).collect();
            v.extend((0..self.files.len()).filter(|&i| !self.files[i].color));
            v
        } else {
            (0..self.files.len()).collect()
        };
        for i in order {
            if let Some(f) = self.font(i, px) {
                if chars.iter().all(|&c| f.has_char(c)) {
                    return i;
                }
            }
        }
        0
    }
}

fn is_invisible_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Characters shown as emoji by default (approximation of the Unicode
/// Emoji_Presentation property: the pictographic blocks plus the BMP symbols
/// that default to emoji style).
pub fn is_emoji_presentation(c: char) -> bool {
    matches!(c as u32,
        0x1F1E6..=0x1F1FF // regional indicators (flags)
        | 0x1F300..=0x1F5FF
        | 0x1F600..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F7E0..=0x1F7EB
        | 0x1F900..=0x1F9FF
        | 0x1FA70..=0x1FAFF
        | 0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE
        | 0x2614 | 0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB
        | 0x26BD | 0x26BE | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E
        | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF
        | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_presentation() {
        assert!(is_emoji_presentation('😀'));
        assert!(is_emoji_presentation('⚡'));
        assert!(!is_emoji_presentation('a'));
        assert!(!is_emoji_presentation('☺')); // text presentation by default
    }
}
