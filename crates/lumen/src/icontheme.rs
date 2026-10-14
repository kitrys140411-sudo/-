//! Application icons from the installed hicolor icon theme (the theme every
//! app installs into, Flatpak's exports included) and `/usr/share/pixmaps`,
//! for apps whose icon Lumen does not draw itself. Only PNG files are used.
//!
//! Lookup and decoding touch the disk, so they run on a background thread;
//! the shell shows a letter tile until the image arrives.

use std::path::{Path, PathBuf};

/// Directories that hold icon themes, most specific first: `~/.icons`, then
/// `icons/` in every XDG data directory (Flatpak exports included).
pub fn base_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".icons")).into_iter().collect();
    v.extend(crate::apps::data_dirs().into_iter().map(|d| d.join("icons")));
    v
}

/// The effective pixel size of a hicolor size directory ("48x48", "32x32@2").
fn dir_size(name: &str) -> Option<u32> {
    let (dims, scale) = match name.split_once('@') {
        Some((d, s)) => (d, s.parse::<u32>().ok()?),
        None => (name, 1),
    };
    let (w, h) = dims.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w == h && w > 0).then_some(w * scale)
}

/// The best PNG for icon `name` at `px` pixels: the smallest image at least
/// that big (downscaling stays sharp), else the biggest there is. Earlier
/// base directories win among equal sizes. An absolute path is used as is.
pub fn find(name: &str, px: u32, bases: &[PathBuf], pixmaps: &Path) -> Option<PathBuf> {
    if name.is_empty() || name.contains("..") {
        return None;
    }
    if name.starts_with('/') {
        let p = PathBuf::from(name);
        return (p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) && p.is_file()).then_some(p);
    }
    if name.contains('/') {
        return None;
    }
    let file = format!("{name}.png");
    let mut best: Option<(u32, PathBuf)> = None;
    for base in bases {
        let theme = base.join("hicolor");
        let Ok(rd) = std::fs::read_dir(&theme) else { continue };
        let mut dirs: Vec<(u32, PathBuf)> = rd
            .flatten()
            .filter_map(|e| Some((dir_size(&e.file_name().to_string_lossy())?, e.path().join("apps").join(&file))))
            .filter(|(_, p)| p.is_file())
            .collect();
        dirs.sort_by_key(|(s, _)| *s);
        for (size, path) in dirs {
            let better = match &best {
                None => true,
                Some((b, _)) if *b >= px => size >= px && size < *b,
                Some((b, _)) => size > *b,
            };
            if better {
                best = Some((size, path));
            }
        }
    }
    best.map(|(_, p)| p).or_else(|| Some(pixmaps.join(&file)).filter(|p| p.is_file()))
}

/// Decode `path` and fit it into a `px` x `px` square (aspect kept,
/// centered), premultiplied RGBA.
pub fn load_file(path: &Path, px: u32) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let (w, h, rgba) = crate::png::decode(&data).map_err(|e| tracing::debug!("icon {}: {e}", path.display())).ok()?;
    let k = px as f64 / w.max(h) as f64;
    let (dw, dh) = (((w as f64 * k).round() as u32).clamp(1, px), ((h as f64 * k).round() as u32).clamp(1, px));
    let fitted = crate::png::resize_premultiplied(&rgba, w, h, dw, dh);
    if (dw, dh) == (px, px) {
        return Some(fitted);
    }
    let mut out = vec![0u8; (px * px * 4) as usize];
    let (ox, oy) = ((px - dw) / 2, (px - dh) / 2);
    for y in 0..dh {
        let src = &fitted[(y * dw * 4) as usize..((y + 1) * dw * 4) as usize];
        let o = (((oy + y) * px + ox) * 4) as usize;
        out[o..o + src.len()].copy_from_slice(src);
    }
    Some(out)
}

/// Look up and load icon `name` at `px` from the installed themes.
pub fn load(name: &str, px: u32) -> Option<Vec<u8>> {
    let path = find(name, px, &base_dirs(), Path::new("/usr/share/pixmaps"))?;
    load_file(&path, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(dir: &Path, size: u32, color: [u8; 4]) {
        std::fs::create_dir_all(dir).unwrap();
        let px = color.repeat((size * size) as usize);
        crate::png::write_file(&dir.join("app.png"), size, size, &px).unwrap();
    }

    #[test]
    fn picks_the_best_size_across_themes() {
        let root = std::env::temp_dir().join(format!("lumen-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (user, flatpak, pixmaps) = (root.join("user"), root.join("flatpak"), root.join("pixmaps"));
        icon(&user.join("hicolor/32x32/apps"), 32, [255, 0, 0, 255]);
        icon(&flatpak.join("hicolor/64x64/apps"), 64, [0, 255, 0, 255]);
        icon(&flatpak.join("hicolor/256x256/apps"), 256, [0, 0, 255, 255]);
        icon(&flatpak.join("hicolor/24x24@2/apps"), 48, [9, 9, 9, 255]);
        let bases = vec![user.clone(), flatpak.clone()];
        let find_at = |px| find("app", px, &bases, &pixmaps).map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned());
        assert_eq!(find_at(48).as_deref(), Some("flatpak/hicolor/24x24@2/apps/app.png"));
        assert_eq!(find_at(56).as_deref(), Some("flatpak/hicolor/64x64/apps/app.png"));
        assert_eq!(find_at(16).as_deref(), Some("user/hicolor/32x32/apps/app.png"));
        assert_eq!(find_at(512).as_deref(), Some("flatpak/hicolor/256x256/apps/app.png"));
        assert_eq!(find("other", 48, &bases, &pixmaps), None);
        icon(&pixmaps, 40, [1, 2, 3, 255]);
        assert_eq!(find("app", 48, &[], &pixmaps), Some(pixmaps.join("app.png")));
        assert_eq!(find("../app", 48, &bases, &pixmaps), None);
        // Loaded and scaled into a square.
        let px = load_file(&flatpak.join("hicolor/64x64/apps/app.png"), 48).unwrap();
        assert_eq!(px.len(), 48 * 48 * 4);
        assert_eq!(&px[..4], &[0, 255, 0, 255]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wide_images_are_centered() {
        let root = std::env::temp_dir().join(format!("lumen-icons-wide-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("wide.png");
        crate::png::write_file(&path, 8, 4, &[255u8, 255, 255, 255].repeat(32)).unwrap();
        let px = load_file(&path, 8).unwrap();
        let alpha: Vec<u8> = px.chunks(4).map(|p| p[3]).collect();
        assert_eq!(&alpha[..8], &[0; 8], "top rows empty");
        assert_eq!(&alpha[16..24], &[255; 8], "middle rows filled");
        assert_eq!(dir_size("32x32@2"), Some(64));
        assert_eq!(dir_size("scalable"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
