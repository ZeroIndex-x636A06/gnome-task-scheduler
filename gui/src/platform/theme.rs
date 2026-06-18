//! Sync dark mode + accent from the host desktop into libadwaita.

use gtk4 as gtk;
use libadwaita as adw;

use super::Desktop;

pub fn apply(desktop: Desktop) {
    let mgr = adw::StyleManager::default();

    // Dark / light preference.
    if let Some(dark) = prefers_dark(desktop) {
        mgr.set_color_scheme(if dark {
            adw::ColorScheme::ForceDark
        } else {
            adw::ColorScheme::ForceLight
        });
    }

    // KDE accent color, if we can find one.
    if desktop == Desktop::Kde {
        if let Some(rgb) = kde_accent() {
            install_accent_css(rgb);
        }
    }
}

fn prefers_dark(desktop: Desktop) -> Option<bool> {
    if desktop == Desktop::Kde {
        if let Some(scheme) = read_ini_value(&kdeglobals_path()?, "General", "ColorScheme") {
            let s = scheme.to_ascii_lowercase();
            if s.contains("dark") {
                return Some(true);
            }
            if s.contains("light") || s.contains("breeze") && !s.contains("dark") {
                return Some(false);
            }
        }
    }
    if matches!(desktop, Desktop::Xfce) {
        if let Ok(out) = std::process::Command::new("xfconf-query")
            .args(["-c", "xsettings", "-p", "/Net/ThemeName"])
            .output()
        {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
                if name.contains("dark") {
                    return Some(true);
                }
            }
        }
    }
    // Fall back to GTK env override.
    if let Ok(theme) = std::env::var("GTK_THEME") {
        if theme.to_ascii_lowercase().contains("dark") {
            return Some(true);
        }
    }
    None
}

fn kdeglobals_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("kdeglobals"))
}

fn kde_accent() -> Option<(u8, u8, u8)> {
    let path = kdeglobals_path()?;
    let raw = read_ini_value(&path, "General", "AccentColor")?;
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    let r = parts[0].parse().ok()?;
    let g = parts[1].parse().ok()?;
    let b = parts[2].parse().ok()?;
    Some((r, g, b))
}

fn read_ini_value(path: &std::path::Path, section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = &line[1..line.len() - 1] == section;
            continue;
        }
        if !in_section || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn install_accent_css(rgb: (u8, u8, u8)) {
    let (r, g, b) = rgb;
    let css = format!(
        "@define-color accent_color rgb({r},{g},{b});\n\
         @define-color accent_bg_color rgb({r},{g},{b});\n\
         @define-color accent_fg_color #ffffff;\n"
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}
