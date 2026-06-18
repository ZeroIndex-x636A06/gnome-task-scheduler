//! Desktop-environment detection and per-DE polish for the GTK4 app.
//!
//! libadwaita owns most of our visual styling; this module's job is to make
//! the app feel less out-of-place on non-GNOME hosts by syncing dark mode
//! and accent color, tweaking header-bar chrome, and layering a small CSS
//! override per desktop. We never try to fake Breeze or Win32 pixel-for-pixel.

pub mod css;
pub mod theme;

use libadwaita as adw;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    Xfce,
    Cinnamon,
    Mate,
    TilingWm,
    Other,
}

use std::sync::OnceLock;
static CURRENT: OnceLock<Desktop> = OnceLock::new();

/// Detect the host desktop once and cache it for subsequent calls.
pub fn current() -> Desktop {
    *CURRENT.get_or_init(detect)
}

pub fn detect() -> Desktop {
    if std::env::var_os("SWAYSOCK").is_some()
        || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        || std::env::var_os("I3SOCK").is_some()
    {
        return Desktop::TilingWm;
    }
    if std::env::var_os("KDE_FULL_SESSION").is_some() {
        return Desktop::Kde;
    }
    let xdg = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if xdg.contains("kde") || xdg.contains("plasma") {
        Desktop::Kde
    } else if xdg.contains("xfce") {
        Desktop::Xfce
    } else if xdg.contains("cinnamon") {
        Desktop::Cinnamon
    } else if xdg.contains("mate") {
        Desktop::Mate
    } else if xdg.contains("gnome") || xdg.contains("unity") {
        Desktop::Gnome
    } else if xdg.contains("sway")
        || xdg.contains("hyprland")
        || xdg.contains("i3")
        || xdg.contains("river")
        || xdg.contains("niri")
    {
        Desktop::TilingWm
    } else {
        Desktop::Other
    }
}

/// Apply per-DE tweaks to an `adw::HeaderBar`. Call once per header bar
/// right after construction.
pub fn style_header(header: &adw::HeaderBar, desktop: Desktop) {
    match desktop {
        Desktop::TilingWm => {
            header.set_show_start_title_buttons(false);
            header.set_show_end_title_buttons(false);
        }
        Desktop::Kde | Desktop::Xfce | Desktop::Cinnamon | Desktop::Mate => {
            header.set_decoration_layout(Some("icon:minimize,maximize,close"));
        }
        Desktop::Gnome | Desktop::Other => {}
    }
}

