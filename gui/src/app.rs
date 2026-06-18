use adw::prelude::*;
use gtk4 as gtk;
use gtk4::{gio, glib};
use libadwaita as adw;

use crate::platform;
use crate::scheduler::{InitSystem, Registry};
use crate::ui::new_task_dialog::present_new_task_dialog;
use crate::ui::task_library::TaskLibrary;

pub fn build_ui(app: &adw::Application, _desktop: platform::Desktop) {
    // Register the project's packaging/ folder as an icon theme search path so
    // the About dialog can find task-scheduler.png (dev-time only; installed
    // builds get the icon via the normal hicolor install).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let packaging = root.join("packaging");
            if packaging.exists() {
                if let Some(display) = gtk::gdk::Display::default() {
                    gtk::IconTheme::for_display(&display)
                        .add_search_path(packaging.to_str().unwrap_or(""));
                }
            }
        }
    }

    let registry = Registry::build();

    let toast_overlay = adw::ToastOverlay::new();

    let library = TaskLibrary::new(registry.clone(), toast_overlay.clone());
    library.refresh();

    let nav_view = adw::NavigationView::new();
    let library_page = adw::NavigationPage::builder()
        .title("Task Library")
        .child(&library.widget())
        .build();
    nav_view.add(&library_page);

    let header = adw::HeaderBar::new();
    platform::style_header(&header, platform::current());

    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh task list"));
    header.pack_start(&refresh_btn);

    // Right side (pack_end: first call = far right, subsequent go left)
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Menu")
        .build();
    header.pack_end(&menu_btn);

    let new_btn = gtk::Button::builder()
        .label("New Task")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    header.pack_end(&new_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if registry.profile.init != InitSystem::Systemd {
        let banner = adw::Banner::new(
            "systemd not detected — DBus event triggers and timer-precise schedules unavailable.",
        );
        banner.set_revealed(true);
        body.append(&banner);
    } else if !registry.profile.daemon_present {
        let banner = adw::Banner::new(
            "Root daemon not running — system-wide tasks disabled. Run ./install.sh.",
        );
        banner.set_revealed(true);
        body.append(&banner);
    }
    body.append(&nav_view);
    toolbar.set_content(Some(&body));
    toast_overlay.set_child(Some(&toolbar));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Task Scheduler")
        .default_width(1100)
        .default_height(720)
        .content(&toast_overlay)
        .build();
    library.set_parent(window.clone());

    // ── App menu ─────────────────────────────────────────────────────────────
    let app_menu = gio::Menu::new();
    app_menu.append(Some("Error Log"), Some("win.error-log"));
    app_menu.append(Some("About Task Scheduler"), Some("win.about"));
    menu_btn.set_menu_model(Some(&app_menu));

    let action_group = gio::SimpleActionGroup::new();

    {
        let window = window.clone();
        let error_log_action = gio::SimpleAction::new("error-log", None);
        error_log_action.connect_activate(move |_, _| show_error_log(&window));
        action_group.add_action(&error_log_action);
    }
    {
        let window = window.clone();
        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(move |_, _| show_about(&window));
        action_group.add_action(&about_action);
    }
    window.insert_action_group("win", Some(&action_group));

    // ── Button callbacks ──────────────────────────────────────────────────────
    {
        let library = library.clone();
        refresh_btn.connect_clicked(move |_| library.refresh());
    }
    {
        let registry = registry.clone();
        let library = library.clone();
        let toast_overlay = toast_overlay.clone();
        let window = window.clone();
        new_btn.connect_clicked(move |_| {
            present_new_task_dialog(
                &window,
                registry.clone(),
                library.clone(),
                toast_overlay.clone(),
            );
        });
    }

    window.present();
}

fn show_error_log(parent: &adw::ApplicationWindow) {
    let entries = crate::error_log::entries();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Error Log")
        .default_width(640)
        .default_height(480)
        .build();

    let header = adw::HeaderBar::new();
    platform::style_header(&header, platform::current());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    if entries.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Errors")
            .description("Errors from task operations will appear here.")
            .icon_name("dialog-information-symbolic")
            .build();
        toolbar.set_content(Some(&status));
    } else {
        let clear_btn = gtk::Button::with_label("Clear");
        clear_btn.add_css_class("destructive-action");
        header.pack_end(&clear_btn);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        for msg in &entries {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(msg))
                .subtitle_lines(0)
                .build();
            let icon = gtk::Image::from_icon_name("dialog-error-symbolic");
            icon.add_css_class("error");
            row.add_prefix(&icon);
            list.append(&row);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&list)
            .build();
        toolbar.set_content(Some(&scroller));

        let dialog_weak = dialog.downgrade();
        clear_btn.connect_clicked(move |_| {
            crate::error_log::clear();
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
    }

    dialog.set_content(Some(&toolbar));
    dialog.present();
}

fn show_about(parent: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Task Scheduler")
        .developer_name("Caleb Jarrell")
        .version(env!("CARGO_PKG_VERSION"))
        .application_icon("task-scheduler")
        .comments("Schedule and manage automated tasks on Linux.")
        .license_type(gtk4::License::MitX11)
        .build();
    dialog.present(Some(parent));
}
