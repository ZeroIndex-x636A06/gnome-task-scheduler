//! Task library — searchable, filterable view of every task on the host.
//!
//! Shows tasks from every backend, tagged Owned (created here) or Foreign
//! (pre-existing on the system). Filter chips let the user narrow by origin,
//! status, and trigger type. Each row has Edit / Toggle / Delete + a menu
//! with revert-to-previous / revert-to-original.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use libadwaita as adw;

use crate::scheduler::{Registry, SnapshotTarget, Task, TaskOrigin, Trigger};
use task_scheduler_core::is_protected_name;

use crate::ui::new_task_dialog::{present_task_editor, EditorMode};

mod imp_task_object {
    use super::*;
    use glib::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct TaskObject {
        pub inner: RefCell<Option<Task>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TaskObject {
        const NAME: &'static str = "TaskSchedulerTaskObject";
        type Type = super::TaskObject;
    }

    impl ObjectImpl for TaskObject {}
}

glib::wrapper! {
    pub struct TaskObject(ObjectSubclass<imp_task_object::TaskObject>);
}

impl TaskObject {
    pub fn new(task: Task) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().inner.borrow_mut() = Some(task);
        obj
    }
    pub fn task(&self) -> Task {
        self.imp().inner.borrow().clone().expect("task set")
    }
}

#[derive(Default, Clone)]
struct FilterState {
    search: String,
    origin: OriginFilter,
    show_enabled: bool,
    show_disabled: bool,
    show_scheduled: bool,
    show_at_boot: bool,
    show_at_login: bool,
    show_on_device: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum OriginFilter {
    #[default]
    All,
    Owned,
    Foreign,
}

#[derive(Clone)]
pub struct TaskLibrary {
    root: gtk::Box,
    store: gtk::gio::ListStore,
    registry: Registry,
    toasts: adw::ToastOverlay,
    parent: Rc<RefCell<Option<adw::ApplicationWindow>>>,
}

impl TaskLibrary {
    pub fn new(registry: Registry, toasts: adw::ToastOverlay) -> Self {
        let store = gtk::gio::ListStore::new::<TaskObject>();

        // Filter bar
        let filter_state = Rc::new(RefCell::new(FilterState {
            origin: OriginFilter::All,
            show_enabled: true,
            show_disabled: true,
            show_scheduled: true,
            show_at_boot: true,
            show_at_login: true,
            show_on_device: true,
            ..Default::default()
        }));

        let filter = {
            let fs = filter_state.clone();
            gtk::CustomFilter::new(move |obj| {
                let Some(t_obj) = obj.downcast_ref::<TaskObject>() else { return true };
                let t = t_obj.task();
                let st = fs.borrow();
                if !st.search.is_empty() {
                    let q = st.search.to_lowercase();
                    if !t.name.to_lowercase().contains(&q)
                        && !t.command.to_lowercase().contains(&q)
                    {
                        return false;
                    }
                }
                match st.origin {
                    OriginFilter::Owned if t.origin != TaskOrigin::Owned => return false,
                    OriginFilter::Foreign if t.origin != TaskOrigin::Foreign => return false,
                    _ => {}
                }
                if t.enabled && !st.show_enabled { return false; }
                if !t.enabled && !st.show_disabled { return false; }
                let pass_trigger = match t.trigger {
                    Trigger::OnCalendar(_) => st.show_scheduled,
                    Trigger::OnBootSec(_) => st.show_at_boot,
                    Trigger::OnLogin => st.show_at_login,
                    Trigger::OnDevice(_) => st.show_on_device,
                };
                if !pass_trigger { return false; }
                true
            })
        };
        let filter_model = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let selection = gtk::SingleSelection::new(Some(filter_model.clone()));

        let column_view = gtk::ColumnView::builder()
            .model(&selection)
            .show_row_separators(true)
            .show_column_separators(true)
            .build();

        column_view.append_column(&text_column("Name", |t| {
            if is_protected_name(&t.name) {
                format!("🔒 {}", t.name)
            } else {
                t.name.clone()
            }
        }));
        column_view.append_column(&text_column("Status", |t| {
            if t.enabled { "Enabled".into() } else { "Disabled".into() }
        }));
        column_view.append_column(&text_column("Next Run", |t| {
            t.next_run.clone().unwrap_or_else(|| "—".into())
        }));
        column_view.append_column(&text_column("Trigger", |t| t.trigger.human()));
        column_view.append_column(&text_column("Command", |t| {
            let c = t.command.clone();
            if c.len() > 60 { format!("{}…", &c[..60]) } else { c }
        }));
        column_view.append_column(&text_column("Backend", |t| t.backend.label().to_string()));
        column_view.append_column(&text_column("Origin", |t| match t.origin {
            TaskOrigin::Owned => "—".to_string(),
            TaskOrigin::Foreign => "system".to_string(),
        }));

        let parent: Rc<RefCell<Option<adw::ApplicationWindow>>> = Rc::new(RefCell::new(None));

        let action_factory = gtk::SignalListItemFactory::new();
        {
            let registry_setup = registry.clone();
            let toasts_setup = toasts.clone();
            let store_setup = store.clone();
            let parent_setup = parent.clone();
            action_factory.connect_setup(move |_, list_item| {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let edit = gtk::Button::from_icon_name("document-edit-symbolic");
                edit.set_tooltip_text(Some("Edit"));
                let toggle = gtk::Button::with_label("Toggle");
                let menu_btn = gtk::Button::from_icon_name("view-more-symbolic");
                menu_btn.set_tooltip_text(Some("More…"));

                let delete = gtk::Button::from_icon_name("user-trash-symbolic");
                delete.add_css_class("destructive-action");
                row.append(&edit);
                row.append(&toggle);
                row.append(&menu_btn);
                row.append(&delete);
                list_item
                    .downcast_ref::<gtk::ListItem>()
                    .unwrap()
                    .set_child(Some(&row));

                let li = list_item.downcast_ref::<gtk::ListItem>().unwrap().clone();

                // Edit
                {
                    let registry = registry_setup.clone();
                    let toasts = toasts_setup.clone();
                    let parent = parent_setup.clone();
                    let store = store_setup.clone();
                    let li = li.clone();
                    edit.connect_clicked(move |_| {
                        let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok())
                        else { return; };
                        let t = obj.task();
                        let Some(window) = parent.borrow().clone() else {
                            toasts.add_toast(adw::Toast::new("Window not ready"));
                            return;
                        };
                        // Build a refreshable TaskLibrary handle for the editor.
                        let lib = TaskLibrary {
                            root: gtk::Box::new(gtk::Orientation::Vertical, 0),
                            store: store.clone(),
                            registry: registry.clone(),
                            toasts: toasts.clone(),
                            parent: parent.clone(),
                        };
                        let open = {
                            let window = window.clone();
                            let registry = registry.clone();
                            let lib = lib.clone();
                            let toasts = toasts.clone();
                            let t = t.clone();
                            move || {
                                present_task_editor(
                                    &window,
                                    registry.clone(),
                                    lib.clone(),
                                    toasts.clone(),
                                    EditorMode::Edit(t.clone()),
                                );
                            }
                        };


                        if t.origin == TaskOrigin::Foreign {
                            // Confirm before editing a system-owned task.
                            let dialog = adw::MessageDialog::builder()
                                .transient_for(&window)
                                .modal(true)
                                .heading("Edit a system task?")
                                .body("This task wasn't created by Task Scheduler. Editing will rewrite its unit/cron/desktop file. You can revert to the original from the task's menu afterward.")
                                .build();
                            dialog.add_response("cancel", "Cancel");
                            dialog.add_response("edit", "Edit");
                            dialog.set_response_appearance("edit", adw::ResponseAppearance::Destructive);
                            dialog.set_default_response(Some("cancel"));
                            let open = std::rc::Rc::new(std::cell::RefCell::new(Some(open)));
                            dialog.connect_response(None, move |dlg, r| {
                                if r == "edit" {
                                    if let Some(f) = open.borrow_mut().take() { f(); }
                                }
                                dlg.close();
                            });
                            dialog.present();
                        } else {
                            open();
                        }
                    });
                }

                // Toggle
                {
                    let registry = registry_setup.clone();
                    let toasts = toasts_setup.clone();
                    let store = store_setup.clone();
                    let li = li.clone();
                    toggle.connect_clicked(move |_| {
                        let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok())
                        else { return; };
                        let t = obj.task();
                        let Some(adapter) = registry.by_backend(t.backend) else {
                            toasts.add_toast(adw::Toast::new("Backend unavailable"));
                            return;
                        };
                        match adapter.toggle_task(&t.name, !t.enabled) {
                            Ok(_) => {
                                toasts.add_toast(adw::Toast::new(&format!(
                                    "{}: {}", t.name,
                                    if !t.enabled { "enabled" } else { "disabled" }
                                )));
                                refresh_store(&store, &registry, &toasts);
                            }
                            Err(e) => {
                                let msg = format!("Error: {e}");
                                crate::error_log::push(&msg);
                                toasts.add_toast(adw::Toast::new(&msg));
                            }
                        }
                    });
                }

                // Delete
                {
                    let registry = registry_setup.clone();
                    let toasts = toasts_setup.clone();
                    let store = store_setup.clone();
                    let parent = parent_setup.clone();
                    let li = li.clone();
                    delete.connect_clicked(move |_| {
                        let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok())
                        else { return; };
                        let t = obj.task();
                        let Some(window) = parent.borrow().clone() else {
                            toasts.add_toast(adw::Toast::new("Window not ready"));
                            return;
                        };
                        let dialog = adw::MessageDialog::builder()
                            .transient_for(&window)
                            .modal(true)
                            .heading("Delete task?")
                            .body(&format!("'{}' will be permanently deleted.", t.name))
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("delete", "Delete");
                        dialog.set_response_appearance(
                            "delete",
                            adw::ResponseAppearance::Destructive,
                        );
                        dialog.set_default_response(Some("cancel"));
                        let registry = registry.clone();
                        let toasts = toasts.clone();
                        let store = store.clone();
                        dialog.connect_response(None, move |dlg, r| {
                            if r == "delete" {
                                let Some(adapter) = registry.by_backend(t.backend) else {
                                    toasts.add_toast(adw::Toast::new("Backend unavailable"));
                                    dlg.close();
                                    return;
                                };
                                match adapter.delete_task(&t.name) {
                                    Ok(_) => {
                                        toasts.add_toast(adw::Toast::new(&format!(
                                            "Deleted {}",
                                            t.name
                                        )));
                                        refresh_store(&store, &registry, &toasts);
                                    }
                                    Err(e) => {
                                        let msg = format!("Error: {e}");
                                        crate::error_log::push(&msg);
                                        toasts.add_toast(adw::Toast::new(&msg));
                                    }
                                }
                            }
                            dlg.close();
                        });
                        dialog.present();
                    });
                }

                // Revert menu
                {
                    let registry = registry_setup.clone();
                    let toasts = toasts_setup.clone();
                    let store = store_setup.clone();
                    let li = li.clone();
                    menu_btn.connect_clicked(move |btn| {
                        let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok())
                        else { return; };
                        let t = obj.task();
                        let popover = gtk::Popover::new();
                        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                        let make_btn = |label: &str| -> gtk::Button {
                            let b = gtk::Button::builder().label(label).build();
                            b.add_css_class("flat");
                            b.set_halign(gtk::Align::Start);
                            b
                        };
                        let prev_btn = make_btn("Revert to last saved");
                        prev_btn.set_sensitive(t.has_snapshot_previous);
                        let orig_btn = make_btn("Revert to original (pre-Task-Scheduler)");
                        orig_btn.set_sensitive(t.has_snapshot_original);
                        menu_box.append(&prev_btn);
                        menu_box.append(&orig_btn);
                        popover.set_child(Some(&menu_box));
                        popover.set_parent(btn);
                        {
                            let registry = registry.clone();
                            let toasts = toasts.clone();
                            let store = store.clone();
                            let t = t.clone();
                            let popover = popover.clone();
                            prev_btn.connect_clicked(move |_| {
                                do_revert(&t, SnapshotTarget::Previous, &registry, &toasts, &store);
                                popover.popdown();
                            });
                        }
                        {
                            let registry = registry.clone();
                            let toasts = toasts.clone();
                            let store = store.clone();
                            let t = t.clone();
                            let popover = popover.clone();
                            orig_btn.connect_clicked(move |_| {
                                do_revert(&t, SnapshotTarget::Original, &registry, &toasts, &store);
                                popover.popdown();
                            });
                        }
                        popover.popup();
                    });
                }
            });
        }

        // Per-row bind: gray out every action button on protected tasks
        // (e.g. `task-scheduler-daemon`) so the user can't break the backend
        // the app itself relies on. Tooltips for non-protected rows are
        // restored to their setup defaults.
        action_factory.connect_bind(|_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok()) else { return; };
            let Some(row) = li.child().and_then(|c| c.downcast::<gtk::Box>().ok()) else { return; };
            let t = obj.task();
            let protected = is_protected_name(&t.name);
            let lock_tip = "This task runs the Task Scheduler backend. \
Editing or stopping it from here would break the app — use `systemctl` if you really need to.";
            // Children order matches setup: edit, toggle, menu_btn, delete.
            let default_tips: [Option<&str>; 4] =
                [Some("Edit"), None, Some("More…"), None];
            let mut child = row.first_child();
            let mut idx = 0usize;
            while let Some(w) = child {
                let next = w.next_sibling();
                if let Some(btn) = w.downcast_ref::<gtk::Button>() {
                    btn.set_sensitive(!protected);
                    if protected {
                        btn.set_tooltip_text(Some(lock_tip));
                    } else {
                        btn.set_tooltip_text(default_tips.get(idx).copied().flatten());
                    }
                    if idx == 1 {
                        btn.set_label(if t.enabled { "Disable" } else { "Enable" });
                    }
                    idx += 1;
                }
                child = next;
            }
        });

        let action_column = gtk::ColumnViewColumn::builder()
            .title("Actions")
            .factory(&action_factory)
            .build();
        column_view.insert_column(0, &action_column);


        let scroller = gtk::ScrolledWindow::builder()
            .hexpand(true).vexpand(true).child(&column_view).build();

        // ---- Filter bar UI ----
        let filter_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        filter_box.set_margin_top(8);
        filter_box.set_margin_bottom(6);
        filter_box.set_margin_start(8);
        filter_box.set_margin_end(8);

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search by name or command…")
            .hexpand(true)
            .build();
        filter_box.append(&search_entry);

        let chip_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);

        // Origin segmented toggle
        let origin_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        origin_box.add_css_class("linked");
        let all_btn = gtk::ToggleButton::with_label("All");
        let owned_btn = gtk::ToggleButton::with_label("Created here");
        let foreign_btn = gtk::ToggleButton::with_label("System");
        all_btn.set_active(true);
        owned_btn.set_group(Some(&all_btn));
        foreign_btn.set_group(Some(&all_btn));
        origin_box.append(&all_btn);
        origin_box.append(&owned_btn);
        origin_box.append(&foreign_btn);
        chip_row.append(&origin_box);

        let sep = gtk::Separator::new(gtk::Orientation::Vertical);
        chip_row.append(&sep);

        let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let enabled_btn = gtk::ToggleButton::with_label("Enabled");
        enabled_btn.set_active(true);
        let disabled_btn = gtk::ToggleButton::with_label("Disabled");
        disabled_btn.set_active(true);
        status_box.append(&enabled_btn);
        status_box.append(&disabled_btn);
        chip_row.append(&status_box);

        let sep2 = gtk::Separator::new(gtk::Orientation::Vertical);
        chip_row.append(&sep2);

        let trig_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let sched_btn = gtk::ToggleButton::with_label("Scheduled");
        sched_btn.set_active(true);
        let boot_btn = gtk::ToggleButton::with_label("At boot");
        boot_btn.set_active(true);
        let login_btn = gtk::ToggleButton::with_label("At login");
        login_btn.set_active(true);
        let dev_btn = gtk::ToggleButton::with_label("On device");
        dev_btn.set_active(true);
        trig_box.append(&sched_btn);
        trig_box.append(&boot_btn);
        trig_box.append(&login_btn);
        trig_box.append(&dev_btn);
        chip_row.append(&trig_box);
        filter_box.append(&chip_row);

        let empty_hint = gtk::Label::builder()
            .label("No tasks match your filters.")
            .margin_top(12).margin_bottom(12)
            .css_classes(vec!["dim-label".to_string()])
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&filter_box);
        root.append(&empty_hint);
        root.append(&scroller);

        let lib = Self {
            root,
            store,
            registry,
            toasts,
            parent,
        };

        // Wire filter widgets.
        {
            let fs = filter_state.clone();
            let f = filter.clone();
            search_entry.connect_search_changed(move |e| {
                fs.borrow_mut().search = e.text().to_string();
                f.changed(gtk::FilterChange::Different);
            });
        }
        let bind_origin = |btn: &gtk::ToggleButton, which: OriginFilter,
                           fs: Rc<RefCell<FilterState>>, f: gtk::CustomFilter| {
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    fs.borrow_mut().origin = which;
                    f.changed(gtk::FilterChange::Different);
                }
            });
        };
        bind_origin(&all_btn, OriginFilter::All, filter_state.clone(), filter.clone());
        bind_origin(&owned_btn, OriginFilter::Owned, filter_state.clone(), filter.clone());
        bind_origin(&foreign_btn, OriginFilter::Foreign, filter_state.clone(), filter.clone());

        let bind_flag = |btn: &gtk::ToggleButton,
                         setter: fn(&mut FilterState, bool),
                         fs: Rc<RefCell<FilterState>>,
                         f: gtk::CustomFilter| {
            btn.connect_toggled(move |b| {
                setter(&mut fs.borrow_mut(), b.is_active());
                f.changed(gtk::FilterChange::Different);
            });
        };
        bind_flag(&enabled_btn, |s, v| s.show_enabled = v, filter_state.clone(), filter.clone());
        bind_flag(&disabled_btn, |s, v| s.show_disabled = v, filter_state.clone(), filter.clone());
        bind_flag(&sched_btn, |s, v| s.show_scheduled = v, filter_state.clone(), filter.clone());
        bind_flag(&boot_btn, |s, v| s.show_at_boot = v, filter_state.clone(), filter.clone());
        bind_flag(&login_btn, |s, v| s.show_at_login = v, filter_state.clone(), filter.clone());
        bind_flag(&dev_btn, |s, v| s.show_on_device = v, filter_state.clone(), filter.clone());

        // Show "no tasks" hint only when the filtered list is empty.
        {
            let empty_hint = empty_hint.clone();
            empty_hint.set_visible(filter_model.n_items() == 0);
            filter_model.connect_items_changed(move |m, _, _, _| {
                empty_hint.set_visible(m.n_items() == 0);
            });
        }

        lib
    }

    pub fn widget(&self) -> gtk::Box { self.root.clone() }

    pub fn refresh(&self) {
        refresh_store(&self.store, &self.registry, &self.toasts);
    }

    /// App.rs calls this after creating the window so per-row Edit can open
    /// a transient dialog with the right parent.
    pub fn set_parent(&self, window: adw::ApplicationWindow) {
        *self.parent.borrow_mut() = Some(window);
    }
}

fn do_revert(
    t: &Task,
    target: SnapshotTarget,
    registry: &Registry,
    toasts: &adw::ToastOverlay,
    store: &gtk::gio::ListStore,
) {
    let Some(adapter) = registry.by_backend(t.backend) else {
        toasts.add_toast(adw::Toast::new("Backend unavailable"));
        return;
    };
    match adapter.revert_task(&t.name, target) {
        Ok(_) => {
            let what = match target {
                SnapshotTarget::Previous => "last saved",
                SnapshotTarget::Original => "original",
            };
            toasts.add_toast(adw::Toast::new(&format!("Reverted {} to {what}", t.name)));
            refresh_store(store, registry, toasts);
        }
        Err(e) => toasts.add_toast(adw::Toast::new(&format!("Revert failed: {e}"))),
    }
}

fn refresh_store(
    store: &gtk::gio::ListStore,
    registry: &Registry,
    toasts: &adw::ToastOverlay,
) {
    store.remove_all();
    let (tasks, errors) = registry.list_all_tasks();
    for t in tasks {
        store.append(&TaskObject::new(t));
    }
    for (backend, err) in errors {
        if matches!(err, crate::scheduler::SchedulerError::Ipc(_)) {
            continue;
        }
        let msg = format!("{}: {err}", backend.label());
        crate::error_log::push(&msg);
        toasts.add_toast(adw::Toast::new(&msg));
    }
}

fn text_column<F>(title: &str, project: F) -> gtk::ColumnViewColumn
where
    F: Fn(&Task) -> String + 'static + Clone,
{
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, list_item| {
        let label = gtk::Label::builder().xalign(0.0).build();
        list_item
            .downcast_ref::<gtk::ListItem>()
            .unwrap()
            .set_child(Some(&label));
    });
    let project_bind = project.clone();
    factory.connect_bind(move |_, list_item| {
        let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(obj) = li.item().and_then(|o| o.downcast::<TaskObject>().ok()) else { return; };
        let Some(label) = li.child().and_then(|c| c.downcast::<gtk::Label>().ok()) else { return; };
        label.set_text(&project_bind(&obj.task()));
    });

    gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .expand(true)
        .build()
}
