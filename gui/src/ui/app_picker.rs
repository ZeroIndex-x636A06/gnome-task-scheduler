//! Browse all installed `.desktop` entries the user can see and pick one to
//! pre-fill the New Task command. Apps are rendered as `adw::ExpanderRow`s
//! that nest any `[Desktop Action ...]` entries underneath. Icons are looked
//! up via the GTK icon theme so themed icons render properly.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;


#[derive(Clone, Debug)]
pub struct DesktopAction {
    pub label: String,
    pub exec: String,
}

#[derive(Clone, Debug)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub exec: String,
    pub icon: Option<String>,
    pub actions: Vec<DesktopAction>,
}

thread_local! {
    static APP_CACHE: RefCell<Option<Vec<DesktopApp>>> = const { RefCell::new(None) };
}

/// Synchronous scan; results are memoized for the lifetime of the process.
pub fn scan_desktop_entries(force: bool) -> Vec<DesktopApp> {
    if !force {
        let cached = APP_CACHE.with(|c| c.borrow().clone());
        if let Some(v) = cached {
            return v;
        }
    }
    let mut by_id: HashMap<String, DesktopApp> = HashMap::new();
    let lang = std::env::var("LANG").unwrap_or_default();

    for dir in xdg_application_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if by_id.contains_key(stem) {
                continue;
            }
            if let Some(app) = parse_desktop_file(&path, stem, &lang) {
                by_id.insert(stem.to_string(), app);
            }
        }
    }

    let mut out: Vec<DesktopApp> = by_id.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    APP_CACHE.with(|c| *c.borrow_mut() = Some(out.clone()));
    out
}

fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        dirs.push(d.join("applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    if let Some(h) = home.as_ref() {
        dirs.push(h.join(".local/share/flatpak/exports/share/applications"));
    }
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

fn parse_desktop_file(path: &Path, stem: &str, lang: &str) -> Option<DesktopApp> {
    let text = fs::read_to_string(path).ok()?;
    // Group sections into separate kv maps so we can pick out
    // [Desktop Entry] and each [Desktop Action ...].
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut current: Option<(String, HashMap<String, String>)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() { continue; }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(prev) = current.take() { sections.push(prev); }
            current = Some((name.to_string(), HashMap::new()));
            continue;
        }
        if let Some((_, kv)) = current.as_mut() {
            if let Some((k, v)) = line.split_once('=') {
                kv.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    if let Some(prev) = current.take() { sections.push(prev); }

    let entry = sections.iter().find(|(n, _)| n == "Desktop Entry")?;
    let kv = &entry.1;
    if kv.get("Type").map(|s| s.as_str()) != Some("Application") { return None; }
    if kv.get("NoDisplay").map(|s| s.as_str()) == Some("true")
        || kv.get("Hidden").map(|s| s.as_str()) == Some("true")
    {
        return None;
    }
    if let Some(tryexec) = kv.get("TryExec") {
        if !tryexec_found(tryexec) { return None; }
    }
    let exec_raw = kv.get("Exec")?.clone();
    let terminal = kv.get("Terminal").map(|s| s.as_str()) == Some("true");
    let exec = clean_exec(&exec_raw, terminal);
    if exec.is_empty() { return None; }
    let name = locale_pick(kv, "Name", lang).unwrap_or_else(|| stem.to_string());
    let comment = locale_pick(kv, "Comment", lang).or_else(|| locale_pick(kv, "GenericName", lang));
    let icon = kv.get("Icon").cloned();

    // Parse Desktop Actions
    let declared: Vec<String> = kv
        .get("Actions")
        .map(|s| {
            s.split(';')
                .filter(|x| !x.trim().is_empty())
                .map(|x| x.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut actions: Vec<DesktopAction> = Vec::new();
    for (sec_name, kv) in &sections {
        let Some(action_id) = sec_name.strip_prefix("Desktop Action ") else { continue };
        if !declared.is_empty() && !declared.iter().any(|d| d == action_id) { continue; }
        let Some(action_exec_raw) = kv.get("Exec") else { continue };
        let action_exec = clean_exec(action_exec_raw, terminal);
        if action_exec.is_empty() { continue; }
        let label = locale_pick(kv, "Name", lang).unwrap_or_else(|| action_id.to_string());
        actions.push(DesktopAction { label, exec: action_exec });
    }

    Some(DesktopApp { id: stem.to_string(), name, comment, exec, icon, actions })
}

fn locale_pick(kv: &HashMap<String, String>, base: &str, lang: &str) -> Option<String> {
    let lang_short = lang.split('.').next().unwrap_or(lang);
    let lang_lang = lang_short.split('_').next().unwrap_or(lang_short);
    if !lang_short.is_empty() {
        if let Some(v) = kv.get(&format!("{base}[{lang_short}]")) {
            return Some(v.clone());
        }
    }
    if !lang_lang.is_empty() {
        if let Some(v) = kv.get(&format!("{base}[{lang_lang}]")) {
            return Some(v.clone());
        }
    }
    kv.get(base).cloned()
}

fn tryexec_found(tryexec: &str) -> bool {
    if tryexec.starts_with('/') {
        return Path::new(tryexec).exists();
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').any(|p| Path::new(p).join(tryexec).exists())
}

fn clean_exec(exec: &str, terminal: bool) -> String {
    let cleaned: Vec<String> = exec
        .split_whitespace()
        .filter(|tok| {
            !matches!(
                *tok,
                "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m"
            )
        })
        .map(|tok| {
            let mut t = tok.to_string();
            for code in ["%f", "%F", "%u", "%U", "%i", "%c", "%k"] {
                t = t.replace(code, "");
            }
            t
        })
        .filter(|s| !s.is_empty())
        .collect();
    if cleaned.is_empty() { return String::new(); }
    let joined = cleaned.join(" ");
    if terminal { format!("x-terminal-emulator -e {joined}") } else { joined }
}

/// Synthesized "pick result" — could be the main app or one of its actions.
fn synth_for_action(app: &DesktopApp, action: &DesktopAction) -> DesktopApp {
    DesktopApp {
        id: format!("{}-{}", app.id, slug(&action.label)),
        name: format!("{} — {}", app.name, action.label),
        comment: app.comment.clone(),
        exec: action.exec.clone(),
        icon: app.icon.clone(),
        actions: Vec::new(),
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Present a modal picker. `on_pick` fires with the chosen app or action.
pub fn present_app_picker(
    parent: &adw::ApplicationWindow,
    on_pick: impl Fn(DesktopApp) + 'static,
) {
    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Choose Application")
        .default_width(560)
        .default_height(680)
        .build();

    let header = adw::HeaderBar::new();
    crate::platform::style_header(&header, crate::platform::current());
    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Rescan applications"));
    header.pack_end(&refresh_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search applications…")
        .build();
    body.append(&search);

    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .overlay_scrolling(false)
        .build();
    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();
    scroller.set_child(Some(&list_box));
    body.append(&scroller);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(6);
    let cancel_btn = gtk::Button::with_label("Cancel");
    actions.append(&cancel_btn);
    body.append(&actions);

    toolbar.set_content(Some(&body));
    window.set_content(Some(&toolbar));

    let apps: Rc<RefCell<Vec<DesktopApp>>> = Rc::new(RefCell::new(scan_desktop_entries(false)));
    let on_pick: Rc<dyn Fn(DesktopApp)> = Rc::new(on_pick);

    populate_rows(&list_box, &apps.borrow(), &window, on_pick.clone());

    // Filter
    {
        let apps = apps.clone();
        let search = search.clone();
        list_box.set_filter_func(move |row| {
            let text = search.text().to_string().to_lowercase();
            if text.is_empty() { return true; }
            let idx = row.index() as usize;
            let apps = apps.borrow();
            let Some(app) = apps.get(idx) else { return false };
            app.name.to_lowercase().contains(&text)
                || app.comment.as_deref().unwrap_or("").to_lowercase().contains(&text)
                || app.id.to_lowercase().contains(&text)
                || app.actions.iter().any(|a| a.label.to_lowercase().contains(&text))
        });
    }
    {
        let list_box = list_box.clone();
        search.connect_search_changed(move |_| list_box.invalidate_filter());
    }

    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }
    {
        let apps = apps.clone();
        let list_box = list_box.clone();
        let window_for_refresh = window.clone();
        let on_pick = on_pick.clone();
        refresh_btn.connect_clicked(move |_| {
            let fresh = scan_desktop_entries(true);
            *apps.borrow_mut() = fresh;
            populate_rows(&list_box, &apps.borrow(), &window_for_refresh, on_pick.clone());
        });
    }

    window.present();
}

fn make_icon(icon_name: Option<&str>) -> gtk::Image {
    let display = gtk::gdk::Display::default();
    let name = icon_name.unwrap_or("application-x-executable");
    let img = if let Some(d) = display {
        let theme = gtk::IconTheme::for_display(&d);
        if theme.has_icon(name) {
            gtk::Image::from_icon_name(name)
        } else {
            gtk::Image::from_icon_name("application-x-executable")
        }
    } else {
        gtk::Image::from_icon_name(name)
    };
    img.set_pixel_size(32);
    img
}

fn populate_rows(
    list_box: &gtk::ListBox,
    apps: &[DesktopApp],
    window: &adw::Window,
    on_pick: Rc<dyn Fn(DesktopApp)>,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    for app in apps {
        if app.actions.is_empty() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&app.name).as_str())
                .build();
            if let Some(c) = &app.comment {
                row.set_subtitle(glib::markup_escape_text(c).as_str());
            }
            row.add_prefix(&make_icon(app.icon.as_deref()));
            row.set_activatable(true);
            {
                let app = app.clone();
                let on_pick = on_pick.clone();
                let window = window.clone();
                row.connect_activated(move |_| {
                    on_pick(app.clone());
                    window.close();
                });
            }
            list_box.append(&row);
        } else {
            let expander = adw::ExpanderRow::builder()
                .title(glib::markup_escape_text(&app.name).as_str())
                .build();
            if let Some(c) = &app.comment {
                expander.set_subtitle(glib::markup_escape_text(c).as_str());
            }
            expander.add_prefix(&make_icon(app.icon.as_deref()));
            // Header itself — clicking activates the main exec.
            let pick_main = gtk::Button::builder()
                .label("Pick")
                .css_classes(vec!["flat".to_string()])
                .valign(gtk::Align::Center)
                .build();
            {
                let app = app.clone();
                let on_pick = on_pick.clone();
                let window = window.clone();
                pick_main.connect_clicked(move |_| {
                    on_pick(app.clone());
                    window.close();
                });
            }
            expander.add_suffix(&pick_main);
            for action in &app.actions {
                let arow = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&action.label).as_str())
                    .subtitle(glib::markup_escape_text(&action.exec).as_str())
                    .build();
                arow.set_activatable(true);
                arow.add_prefix(&make_icon(app.icon.as_deref()));
                let synth = synth_for_action(app, action);
                let on_pick = on_pick.clone();
                let window = window.clone();
                arow.connect_activated(move |_| {
                    on_pick(synth.clone());
                    window.close();
                });
                expander.add_row(&arow);
            }
            list_box.append(&expander);
        }
    }
}


