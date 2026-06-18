//! Per-desktop CSS overrides layered on top of Adwaita.

use gtk4 as gtk;

use super::Desktop;

pub fn install(desktop: Desktop) {
    let css = match desktop {
        Desktop::Kde => KDE_CSS,
        Desktop::Xfce | Desktop::Cinnamon | Desktop::Mate => TRADITIONAL_CSS,
        Desktop::TilingWm => TILING_CSS,
        Desktop::Gnome | Desktop::Other => return,
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

const KDE_CSS: &str = "
headerbar { min-height: 38px; padding: 2px 4px; }
window { border-radius: 0; }
window.csd { box-shadow: 0 0 0 1px alpha(@borders, 0.6); }
button.suggested-action { border-radius: 3px; }
";

const TRADITIONAL_CSS: &str = "
headerbar { min-height: 38px; padding: 2px 4px; }
window { border-radius: 0; }
window.csd { box-shadow: 0 0 0 1px alpha(@borders, 0.6); }
button { border-radius: 3px; }
button.suggested-action, button.destructive-action { border-radius: 3px; }
";

const TILING_CSS: &str = "
window, window.csd { border-radius: 0; box-shadow: none; margin: 0; }
headerbar { min-height: 36px; padding: 0 4px; }
headerbar windowcontrols { opacity: 0; min-width: 0; }
row { min-height: 36px; }
";
