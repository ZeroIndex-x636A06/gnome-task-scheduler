//! Create / edit dialog. In edit mode the backend and name fields are
//! locked, the schedule builder is pre-populated from the task's trigger,
//! and a warning is shown up-front when editing a foreign (system) task.

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::scheduler::{Backend, Registry, Task, TaskOrigin};
use crate::ui::app_picker::present_app_picker;
use crate::ui::schedule_builder::ScheduleBuilder;
use crate::ui::task_library::TaskLibrary;

pub enum EditorMode {
    Create,
    Edit(Task),
}

pub fn present_new_task_dialog(
    parent: &adw::ApplicationWindow,
    registry: Registry,
    library: TaskLibrary,
    toasts: adw::ToastOverlay,
) {
    present_task_editor(parent, registry, library, toasts, EditorMode::Create);
}

pub fn present_task_editor(
    parent: &adw::ApplicationWindow,
    registry: Registry,
    library: TaskLibrary,
    toasts: adw::ToastOverlay,
    mode: EditorMode,
) {
    let edit_task = match &mode {
        EditorMode::Edit(t) => Some(t.clone()),
        EditorMode::Create => None,
    };

    let title = if edit_task.is_some() { "Edit Task" } else { "New Task" };
    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(560)
        .default_height(640)
        .build();


    // Backend picker — only options that the host actually supports.
    let backends: Vec<Backend> = registry.available().iter().map(|a| a.backend()).collect();
    if backends.is_empty() {
        toasts.add_toast(adw::Toast::new("No task backends available on this host."));
        return;
    }
    // In edit mode, restrict to the task's existing backend.
    let backends: Vec<Backend> = if let Some(t) = &edit_task {
        if backends.contains(&t.backend) { vec![t.backend] } else { backends }
    } else {
        backends
    };
    let labels: Vec<&str> = backends.iter().map(|b| b.label()).collect();
    let backend_model = gtk::StringList::new(&labels);
    let backend_row = adw::ComboRow::builder()
        .title("Backend")
        .model(&backend_model)
        .build();
    backend_row.set_sensitive(edit_task.is_none());

    let backend_desc_row = adw::ActionRow::builder()
        .title("")
        .subtitle(backends[0].description())
        .activatable(false)
        .build();
    let info_icon = gtk::Image::from_icon_name("dialog-information-symbolic");
    info_icon.add_css_class("dim-label");
    backend_desc_row.add_prefix(&info_icon);

    let system_warn_row = adw::ActionRow::builder()
        .title("GUI apps won't work here")
        .subtitle(
            "System tasks run as root with no display session. \
             Use systemd (user) or cron (user) for browsers, editors, or anything with a window.",
        )
        .activatable(false)
        .build();
    let warn_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    warn_icon.add_css_class("warning");
    system_warn_row.add_prefix(&warn_icon);
    system_warn_row.set_visible(backends[0].requires_daemon());


    let name_row = adw::EntryRow::builder()
        .title("Task Name")
        .build();
    if let Some(t) = &edit_task {
        name_row.set_text(&t.name);
        name_row.set_sensitive(false);
    } else {
        // Real-time validation: highlight the field when the name would be rejected.
        name_row.connect_changed(|row| {
            let text = row.text();
            let t = text.trim();
            if t.is_empty() || task_scheduler_core::validate_name(t).is_ok() {
                row.remove_css_class("error");
                row.set_tooltip_text(None);
            } else {
                row.add_css_class("error");
                row.set_tooltip_text(Some(
                    "Only letters, digits, hyphens (-) and underscores (_) allowed — no spaces",
                ));
            }
        });
    }

    let command_row = adw::EntryRow::builder()
        .title("Command (absolute path recommended)")
        .build();
    if let Some(t) = &edit_task {
        command_row.set_text(&t.command);
    }
    let pick_app_btn = gtk::Button::builder()
        .icon_name("application-x-executable-symbolic")
        .tooltip_text("Choose an installed application…")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat".to_string()])
        .build();
    command_row.add_suffix(&pick_app_btn);

    let enable_row = adw::SwitchRow::builder()
        .title("Enable immediately")
        .subtitle("Start the task right after it's created")
        .active(true)
        .build();
    enable_row.set_visible(edit_task.is_none());

    let lifecycle_model = gtk::StringList::new(&[
        "Off — keep running on schedule",
        "Disable after first run",
        "Delete after first run",
    ]);
    let lifecycle_row = adw::ComboRow::builder()
        .title("After it runs")
        .subtitle("Choose to auto-disable or auto-delete this task once it executes")
        .model(&lifecycle_model)
        .selected(0)
        .build();
    lifecycle_row.set_visible(edit_task.is_none());

    let details = adw::PreferencesGroup::builder()
        .title("Task Details")
        .build();
    details.add(&backend_row);
    details.add(&backend_desc_row);
    details.add(&system_warn_row);
    details.add(&name_row);
    details.add(&command_row);
    details.add(&enable_row);
    details.add(&lifecycle_row);




    let schedule_builder = ScheduleBuilder::new();
    {
        let initial_caps = registry
            .by_backend(backends[0])
            .map(|a| a.capabilities())
            .unwrap_or(task_scheduler_core::Capabilities::none());
        schedule_builder.set_backend(backends[0], initial_caps);
    }
    schedule_builder.bind_device_picker(parent.clone());
    // Pre-populate in edit mode.
    if let Some(t) = &edit_task {
        schedule_builder.load(&t.trigger);
    }

    let page = adw::PreferencesPage::new();

    // Warning banner for foreign-task edits.
    if let Some(t) = &edit_task {
        if t.origin == TaskOrigin::Foreign {
            let banner = adw::PreferencesGroup::builder().build();
            let warn = adw::ActionRow::builder()
                .title("This task wasn't created by Task Scheduler")
                .subtitle("Saving will rewrite its unit/crontab/desktop file. You can revert later from the task's menu.")
                .build();
            let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
            icon.add_css_class("warning");
            warn.add_prefix(&icon);
            banner.add(&warn);
            page.add(&banner);
        }
    }
    page.add(&details);

    // Schedule lives in its own group so it shares the page's column width
    // and never gets visually clipped above the Command row.
    let schedule_group = adw::PreferencesGroup::builder()
        .title("Schedule")
        .build();
    schedule_group.add(schedule_builder.widget());
    page.add(&schedule_group);


    // App picker wiring.
    {
        let parent_for_picker = parent.clone();
        let command_row = command_row.clone();
        let name_row = name_row.clone();
        let edit_locked_name = edit_task.is_some();
        pick_app_btn.connect_clicked(move |_| {
            let command_row = command_row.clone();
            let name_row = name_row.clone();
            present_app_picker(&parent_for_picker, move |app| {
                command_row.set_text(&app.exec);
                if !edit_locked_name && name_row.text().is_empty() {
                    let slug: String = app
                        .name
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect();
                    let slug = slug
                        .split('-')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("-");
                    name_row.set_text(&slug);
                }
            });
        });
    }

    // Backend change → update subtitle + reshape schedule presets (create only).
    if edit_task.is_none() {
        let registry = registry.clone();
        let backends = backends.clone();
        let backend_desc_row = backend_desc_row.clone();
        let system_warn_row = system_warn_row.clone();
        let schedule_builder = schedule_builder.clone();
        backend_row.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            let backend = backends[idx.min(backends.len().saturating_sub(1))];
            backend_desc_row.set_subtitle(backend.description());
            system_warn_row.set_visible(backend.requires_daemon());
            let caps = registry
                .by_backend(backend)
                .map(|a| a.capabilities())
                .unwrap_or(task_scheduler_core::Capabilities::none());
            schedule_builder.set_backend(backend, caps);
        });
    }


    let cancel_btn = gtk::Button::with_label("Cancel");
    let action_label = if edit_task.is_some() { "Save" } else { "Create" };
    let create_btn = gtk::Button::builder()
        .label(action_label)
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(12);
    actions.set_margin_bottom(12);
    actions.set_margin_start(12);
    actions.set_margin_end(12);
    actions.append(&cancel_btn);
    actions.append(&create_btn);

    let header = adw::HeaderBar::new();
    crate::platform::style_header(&header, crate::platform::current());
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&page)
        .build();
    body.append(&scroller);
    body.append(&actions);
    toolbar.set_content(Some(&body));

    // Give the dialog its own toast overlay so error messages appear in front
    // of the dialog window rather than behind it on the main window.
    let dialog_toasts = adw::ToastOverlay::new();
    dialog_toasts.set_child(Some(&toolbar));
    window.set_content(Some(&dialog_toasts));


    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }

    {
        let window = window.clone();
        let library = library.clone();
        let toasts = toasts.clone();
        let dialog_toasts = dialog_toasts.clone();
        let registry = registry.clone();
        let name_row = name_row.clone();
        let command_row = command_row.clone();
        let backend_row = backend_row.clone();
        let backends = backends.clone();
        let schedule_builder = schedule_builder.clone();
        let enable_row = enable_row.clone();
        let lifecycle_row = lifecycle_row.clone();
        let is_edit = edit_task.is_some();

        create_btn.connect_clicked(move |_| {
            let name = name_row.text().trim().to_string();
            let command = command_row.text().trim().to_string();
            if name.is_empty() || command.is_empty() {
                dialog_toasts.add_toast(adw::Toast::new("Name and command are required"));
                return;
            }
            if let Err(_) = task_scheduler_core::validate_name(&name) {
                dialog_toasts.add_toast(adw::Toast::new(
                    "Task name may only contain letters, digits, hyphens (-) and underscores (_) — no spaces",
                ));
                return;
            }
            let backend = backends[(backend_row.selected() as usize)
                .min(backends.len().saturating_sub(1))];
            let trigger = match schedule_builder.trigger() {
                Ok(t) => t,
                Err(msg) => {
                    dialog_toasts.add_toast(adw::Toast::new(&msg));
                    return;
                }
            };
            let want_enabled = is_edit || enable_row.is_active();
            let lifecycle = if is_edit {
                task_scheduler_core::Lifecycle::Persistent
            } else {
                match lifecycle_row.selected() {
                    1 => task_scheduler_core::Lifecycle::DisableAfterRun,
                    2 => task_scheduler_core::Lifecycle::DeleteAfterRun,
                    _ => task_scheduler_core::Lifecycle::Persistent,
                }
            };
            let task = Task {
                name: name.clone(),
                command,
                trigger,
                enabled: want_enabled,
                next_run: None,
                scope: if backend.requires_daemon() {
                    task_scheduler_core::Scope::System
                } else {
                    task_scheduler_core::Scope::User
                },
                backend,
                origin: task_scheduler_core::TaskOrigin::Owned,
                lifecycle,
                has_snapshot_previous: false,
                has_snapshot_original: false,
            };

            let Some(adapter) = registry.by_backend(backend) else {
                dialog_toasts.add_toast(adw::Toast::new("Backend no longer available"));
                return;
            };

            if !adapter.capabilities().supports(&task.trigger) {
                let msg = format!(
                    "{} doesn't support {} triggers",
                    backend.label(),
                    task.trigger.kind()
                );
                dialog_toasts.add_toast(adw::Toast::new(&msg));
                return;
            }
            // For both create and edit we call create_task — adapters snapshot
            // the previous on-disk state and then overwrite.
            match adapter.create_task(&task) {
                Ok(_) => {
                    // Honor the "Enable immediately" switch on create. In edit
                    // mode the row is hidden and the enable state stays as-is.
                    if !is_edit && !enable_row.is_active() {
                        let _ = adapter.toggle_task(&name, false);
                    }
                    let verb = if is_edit { "Saved" } else { "Created" };
                    toasts.add_toast(adw::Toast::new(&format!(
                        "{verb} {} task '{name}'",
                        backend.label()
                    )));
                    library.refresh();
                    window.close();
                }
                Err(e) => {
                    let msg = format!("Error: {e}");
                    crate::error_log::push(&msg);
                    dialog_toasts.add_toast(adw::Toast::new(&msg));
                }
            }
        });
    }

    window.present();
}
