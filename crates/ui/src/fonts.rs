//! Font setup. Uses fonts already installed on the system (Segoe UI on Windows, DejaVu/Noto
//! on Linux) for proper bold/italic text; falls back to egui's bundled fonts.

use std::path::Path;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

use crate::theme::{BOLD, BOLD_ITALIC, ITALIC};

struct Candidates {
    regular: &'static [&'static str],
    bold: &'static [&'static str],
    italic: &'static [&'static str],
    bold_italic: &'static [&'static str],
    mono: &'static [&'static str],
    symbols: &'static [&'static str],
}

#[cfg(windows)]
const FONTS: Candidates = Candidates {
    regular: &["C:\\Windows\\Fonts\\segoeui.ttf"],
    bold: &["C:\\Windows\\Fonts\\segoeuib.ttf"],
    italic: &["C:\\Windows\\Fonts\\segoeuii.ttf"],
    bold_italic: &["C:\\Windows\\Fonts\\segoeuiz.ttf"],
    mono: &["C:\\Windows\\Fonts\\consola.ttf"],
    symbols: &["C:\\Windows\\Fonts\\seguisym.ttf"],
};

#[cfg(not(windows))]
const FONTS: Candidates = Candidates {
    regular: &[
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ],
    bold: &[
        "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    ],
    italic: &["/usr/share/fonts/truetype/noto/NotoSans-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf"],
    bold_italic: &[
        "/usr/share/fonts/truetype/noto/NotoSans-BoldItalic.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
    ],
    mono: &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"],
    symbols: &[],
};

fn load_first(paths: &[&str]) -> Option<Arc<FontData>> {
    paths.iter().find_map(|p| {
        let path = Path::new(p);
        std::fs::read(path).ok().map(|bytes| Arc::new(FontData::from_owned(bytes)))
    })
}

/// Build font definitions. `use_system` = false gives a deterministic setup (tests, snapshots).
pub fn definitions(use_system: bool) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let base_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let base_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    let add = |defs: &mut FontDefinitions, name: &str, data: Option<Arc<FontData>>| -> Option<String> {
        data.map(|d| {
            defs.font_data.insert(name.to_string(), d);
            name.to_string()
        })
    };

    let (regular, bold, italic, bold_italic, mono, symbols) = if use_system {
        (
            add(&mut defs, "sys-regular", load_first(FONTS.regular)),
            add(&mut defs, "sys-bold", load_first(FONTS.bold)),
            add(&mut defs, "sys-italic", load_first(FONTS.italic)),
            add(&mut defs, "sys-bold-italic", load_first(FONTS.bold_italic)),
            add(&mut defs, "sys-mono", load_first(FONTS.mono)),
            add(&mut defs, "sys-symbols", load_first(FONTS.symbols)),
        )
    } else {
        (None, None, None, None, None, None)
    };

    // Primary face first, then egui's bundled fonts (which carry emoji/icon fallbacks).
    let chain = |primary: Option<&String>| -> Vec<String> {
        let mut v: Vec<String> = primary.cloned().into_iter().collect();
        v.extend(base_prop.iter().cloned());
        v.extend(symbols.iter().cloned());
        v
    };
    let prop = chain(regular.as_ref());
    defs.families.insert(FontFamily::Proportional, prop.clone());
    defs.families.insert(FontFamily::Name(BOLD.into()), chain(bold.as_ref().or(regular.as_ref())));
    defs.families.insert(FontFamily::Name(ITALIC.into()), chain(italic.as_ref().or(regular.as_ref())));
    defs.families
        .insert(FontFamily::Name(BOLD_ITALIC.into()), chain(bold_italic.as_ref().or(bold.as_ref()).or(regular.as_ref())));
    let mut m: Vec<String> = mono.into_iter().collect();
    m.extend(base_mono);
    m.extend(prop);
    defs.families.insert(FontFamily::Monospace, m);
    defs
}
