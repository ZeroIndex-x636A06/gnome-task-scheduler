//! Friendly schedule builder: presets backed by spinners, day toggles, and a
//! calendar widget, with an "Advanced" escape hatch for raw expressions.
//!
//! Produces a `Trigger` shaped for the active backend — systemd backends get
//! `OnCalendar=` strings; cron backends get `cron:` literal 5-field
//! expressions so the cron adapter does not have to translate.

use std::cell::{Cell, RefCell};
use std::rc::Rc;


use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use task_scheduler_core::{Backend, Capabilities, DeviceAction, DeviceMatch, Trigger};

use crate::ui::device_picker::present_device_picker;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Preset {
    Daily,
    Weekly,
    Monthly,
    EveryN,
    OneTime,
    AtBoot,
    AtLogin,
    OnDevice,
    Advanced,
}

impl Preset {
    fn label(self) -> &'static str {
        match self {
            Preset::Daily => "Every day at…",
            Preset::Weekly => "On selected weekdays at…",
            Preset::Monthly => "Monthly on day…",
            Preset::EveryN => "Every N minutes/hours",
            Preset::OneTime => "Once on a specific date",
            Preset::AtBoot => "After system boot",
            Preset::AtLogin => "When I log in",
            Preset::OnDevice => "When a device is connected",
            Preset::Advanced => "Advanced (raw expression)",
        }
    }
    fn stack_name(self) -> &'static str {
        match self {
            Preset::Daily => "daily",
            Preset::Weekly => "weekly",
            Preset::Monthly => "monthly",
            Preset::EveryN => "every_n",
            Preset::OneTime => "one_time",
            Preset::AtBoot => "at_boot",
            Preset::AtLogin => "at_login",
            Preset::OnDevice => "on_device",
            Preset::Advanced => "advanced",
        }
    }
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY_LABELS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

struct Inner {
    backend: Backend,
    caps: Capabilities,
    presets: Vec<Preset>,
}

#[derive(Clone)]
pub struct ScheduleBuilder {
    root: gtk::Box,
    preset_dropdown: gtk::DropDown,
    preset_model: gtk::StringList,
    stack: gtk::Stack,

    // 12h / 24h display mode toggle (default 24h)
    twelve_h: Rc<Cell<bool>>,

    // Daily
    daily_hour: gtk::SpinButton,
    daily_minute: gtk::SpinButton,
    daily_pm: gtk::ToggleButton,
    // Weekly
    weekly_toggles: [gtk::ToggleButton; 7],
    weekly_hour: gtk::SpinButton,
    weekly_minute: gtk::SpinButton,
    weekly_pm: gtk::ToggleButton,
    // Monthly
    monthly_day: gtk::SpinButton,
    monthly_hour: gtk::SpinButton,
    monthly_minute: gtk::SpinButton,
    monthly_pm: gtk::ToggleButton,
    // Every-N
    every_n_value: gtk::SpinButton,
    every_n_unit: gtk::DropDown, // 0 = minutes, 1 = hours
    // One-time
    one_time_calendar: gtk::Calendar,
    one_time_hour: gtk::SpinButton,
    one_time_minute: gtk::SpinButton,
    one_time_pm: gtk::ToggleButton,
    // At boot
    at_boot_value: gtk::SpinButton,
    at_boot_unit: gtk::DropDown, // 0 = seconds, 1 = minutes
    // On device
    on_device_event: gtk::DropDown, // 0 = Connect, 1 = Disconnect, 2 = Change
    on_device_pick_btn: gtk::Button,
    on_device_summary: gtk::Label,
    on_device_subsystem: gtk::Entry,
    on_device_vendor: gtk::Entry,
    on_device_product: gtk::Entry,
    on_device_kernel: gtk::Entry,
    on_device_label: Rc<RefCell<Option<String>>>,
    // Advanced
    advanced_entry: gtk::Entry,

    inner: Rc<RefCell<Inner>>,
}


impl ScheduleBuilder {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_valign(gtk::Align::Start);
        root.set_vexpand(false);

        // Header row: "When should it run?" dropdown + 12h/24h toggle.
        let preset_row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let preset_label = gtk::Label::new(Some("When should it run?"));
        let preset_model = gtk::StringList::new(&[]);
        let preset_dropdown = gtk::DropDown::builder()
            .model(&preset_model)
            .hexpand(true)
            .build();
        let twelve_toggle = gtk::ToggleButton::builder()
            .label("12h")
            .tooltip_text("Toggle between 24-hour and 12-hour time")
            .valign(gtk::Align::Center)
            .build();
        preset_row_box.append(&preset_label);
        preset_row_box.append(&preset_dropdown);
        preset_row_box.append(&twelve_toggle);
        root.append(&preset_row_box);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .margin_top(6)
            .margin_bottom(6)
            .vexpand(false)
            .build();

        // ---- Daily ----
        let (daily_box, daily_hour, daily_minute, daily_pm) = time_row("Time of day");
        daily_box.set_valign(gtk::Align::Start);
        stack.add_named(&daily_box, Some(Preset::Daily.stack_name()));

        // ---- Weekly ----
        let weekly_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        weekly_box.set_valign(gtk::Align::Start);
        let day_label = gtk::Label::builder()
            .label("Days of the week")
            .halign(gtk::Align::Start)
            .build();
        let toggles_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toggles_box.add_css_class("linked");
        toggles_box.set_halign(gtk::Align::Start);
        let weekly_toggles: [gtk::ToggleButton; 7] = std::array::from_fn(|i| {
            let b = gtk::ToggleButton::builder()
                .label(WEEKDAY_LABELS[i])
                .tooltip_text(WEEKDAYS[i])
                .build();
            if i < 5 { b.set_active(true); }
            toggles_box.append(&b);
            b
        });
        let (weekly_time_box, weekly_hour, weekly_minute, weekly_pm) = time_row("At time");
        weekly_box.append(&day_label);
        weekly_box.append(&toggles_box);
        weekly_box.append(&weekly_time_box);
        stack.add_named(&weekly_box, Some(Preset::Weekly.stack_name()));

        // ---- Monthly ----
        let monthly_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        monthly_box.set_valign(gtk::Align::Start);
        let dom_row = labeled_spin("Day of month", 1.0, 31.0, 1.0);
        let monthly_day = dom_row.1;
        monthly_box.append(&dom_row.0);
        let (monthly_time_box, monthly_hour, monthly_minute, monthly_pm) = time_row("At time");
        monthly_box.append(&monthly_time_box);
        stack.add_named(&monthly_box, Some(Preset::Monthly.stack_name()));

        // ---- Every N ----
        let every_n_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        every_n_box.set_halign(gtk::Align::Start);
        every_n_box.set_valign(gtk::Align::Start);
        let lbl = gtk::Label::new(Some("Every"));
        let every_n_value = gtk::SpinButton::with_range(1.0, 1440.0, 1.0);
        every_n_value.set_value(15.0);
        every_n_value.set_valign(gtk::Align::Center);
        every_n_value.set_vexpand(false);
        let every_n_unit = gtk::DropDown::from_strings(&["minutes", "hours"]);
        every_n_unit.set_valign(gtk::Align::Center);
        every_n_box.append(&lbl);
        every_n_box.append(&every_n_value);
        every_n_box.append(&every_n_unit);
        stack.add_named(&every_n_box, Some(Preset::EveryN.stack_name()));

        // ---- One time ----
        let one_time_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        one_time_box.set_valign(gtk::Align::Start);
        let one_time_calendar = gtk::Calendar::new();
        one_time_calendar.set_vexpand(false);
        one_time_calendar.set_halign(gtk::Align::Start);
        let (one_time_time_box, one_time_hour, one_time_minute, one_time_pm) = time_row("At time");
        one_time_box.append(&one_time_calendar);
        one_time_box.append(&one_time_time_box);
        stack.add_named(&one_time_box, Some(Preset::OneTime.stack_name()));


        // ---- At boot ----
        let at_boot_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        at_boot_box.set_halign(gtk::Align::Start);
        let lbl_b = gtk::Label::new(Some("Wait"));
        let at_boot_value = gtk::SpinButton::with_range(0.0, 86400.0, 1.0);
        at_boot_value.set_value(1.0);
        let at_boot_unit = gtk::DropDown::from_strings(&["seconds", "minutes"]);
        at_boot_unit.set_selected(1);
        let lbl_b2 = gtk::Label::new(Some("after boot"));
        at_boot_box.append(&lbl_b);
        at_boot_box.append(&at_boot_value);
        at_boot_box.append(&at_boot_unit);
        at_boot_box.append(&lbl_b2);
        stack.add_named(&at_boot_box, Some(Preset::AtBoot.stack_name()));

        // ---- At login ----
        let at_login_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let info = gtk::Label::builder()
            .label("Runs once when your desktop session starts.")
            .halign(gtk::Align::Start)
            .css_classes(vec!["dim-label".to_string()])
            .build();
        at_login_box.append(&info);
        stack.add_named(&at_login_box, Some(Preset::AtLogin.stack_name()));

        // ---- On device ----
        let on_device_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let event_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        event_row.append(&gtk::Label::new(Some("Event")));
        let on_device_event = gtk::DropDown::from_strings(&[
            "Connect (device plugged in)",
            "Disconnect (device removed)",
            "Change (state changed)",
        ]);
        event_row.append(&on_device_event);
        on_device_box.append(&event_row);

        let pick_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let on_device_pick_btn = gtk::Button::builder()
            .label("Pick device…")
            .css_classes(vec!["pill".to_string()])
            .build();
        let on_device_summary = gtk::Label::builder()
            .label("(no device picked)")
            .halign(gtk::Align::Start)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(vec!["dim-label".to_string()])
            .build();
        pick_row.append(&on_device_pick_btn);
        pick_row.append(&on_device_summary);
        on_device_box.append(&pick_row);

        let custom_expander = gtk::Expander::builder()
            .label("Custom match…")
            .expanded(false)
            .build();
        let custom_grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(8)
            .margin_top(6)
            .build();
        let on_device_subsystem = gtk::Entry::builder()
            .placeholder_text("usb, block, input, net, …")
            .hexpand(true)
            .build();
        let on_device_vendor = gtk::Entry::builder()
            .placeholder_text("idVendor (e.g. 1d6b)")
            .build();
        let on_device_product = gtk::Entry::builder()
            .placeholder_text("idProduct (e.g. 0002)")
            .build();
        let on_device_kernel = gtk::Entry::builder()
            .placeholder_text("kernel name pattern (e.g. sd?1)")
            .hexpand(true)
            .build();
        custom_grid.attach(&gtk::Label::builder().label("Subsystem").halign(gtk::Align::End).build(), 0, 0, 1, 1);
        custom_grid.attach(&on_device_subsystem, 1, 0, 3, 1);
        custom_grid.attach(&gtk::Label::builder().label("Vendor").halign(gtk::Align::End).build(), 0, 1, 1, 1);
        custom_grid.attach(&on_device_vendor, 1, 1, 1, 1);
        custom_grid.attach(&gtk::Label::builder().label("Product").halign(gtk::Align::End).build(), 2, 1, 1, 1);
        custom_grid.attach(&on_device_product, 3, 1, 1, 1);
        custom_grid.attach(&gtk::Label::builder().label("Kernel").halign(gtk::Align::End).build(), 0, 2, 1, 1);
        custom_grid.attach(&on_device_kernel, 1, 2, 3, 1);
        custom_expander.set_child(Some(&custom_grid));
        on_device_box.append(&custom_expander);
        stack.add_named(&on_device_box, Some(Preset::OnDevice.stack_name()));

        // ---- Advanced ----
        let advanced_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let adv_label = gtk::Label::builder()
            .label("Raw systemd OnCalendar / cron expression (prefix cron expressions with 'cron:')")
            .wrap(true)
            .halign(gtk::Align::Start)
            .css_classes(vec!["dim-label".to_string()])
            .build();
        let advanced_entry = gtk::Entry::builder()
            .placeholder_text("daily, *-*-* 09:00:00, cron:*/10 * * * *")
            .build();
        advanced_box.append(&adv_label);
        advanced_box.append(&advanced_entry);
        stack.add_named(&advanced_box, Some(Preset::Advanced.stack_name()));

        root.append(&stack);

        let twelve_h = Rc::new(Cell::new(false));

        let s = Self {
            root: root.clone(),
            preset_dropdown: preset_dropdown.clone(),
            preset_model,
            stack: stack.clone(),
            twelve_h: twelve_h.clone(),
            daily_hour, daily_minute, daily_pm,
            weekly_toggles, weekly_hour, weekly_minute, weekly_pm,
            monthly_day, monthly_hour, monthly_minute, monthly_pm,
            every_n_value, every_n_unit,
            one_time_calendar, one_time_hour, one_time_minute, one_time_pm,
            at_boot_value, at_boot_unit,
            on_device_event,
            on_device_pick_btn,
            on_device_summary,
            on_device_subsystem,
            on_device_vendor,
            on_device_product,
            on_device_kernel,
            on_device_label: Rc::new(RefCell::new(None)),
            advanced_entry,
            inner: Rc::new(RefCell::new(Inner {
                backend: Backend::SystemdUser,
                caps: Capabilities::none(),
                presets: Vec::new(),
            })),
        };

        // Switch stack pages reactively.
        {
            let s = s.clone();
            preset_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                let presets = s.inner.borrow().presets.clone();
                if let Some(p) = presets.get(idx) {
                    s.stack.set_visible_child_name(p.stack_name());
                }
            });
        }

        // 12h / 24h toggle reshapes every hour spinner in place.
        {
            let s = s.clone();
            twelve_toggle.connect_toggled(move |btn| {
                s.apply_time_mode(btn.is_active());
            });
        }

        s
    }


    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Read the 24-hour value backing a (hour-spin, pm-toggle) pair, honoring
    /// the current 12h / 24h display mode.
    fn read_hour(&self, spin: &gtk::SpinButton, pm: &gtk::ToggleButton) -> u32 {
        let raw = spin.value() as u32;
        if self.twelve_h.get() {
            let h12 = if raw == 12 { 0 } else { raw.min(12) };
            if pm.is_active() { h12 + 12 } else { h12 }
        } else {
            raw.min(23)
        }
    }

    /// Push a 24-hour value into a (spin, pm) pair, picking AM/PM as needed.
    fn set_hour_24(&self, spin: &gtk::SpinButton, pm: &gtk::ToggleButton, h24: u32) {
        let h24 = h24.min(23);
        if self.twelve_h.get() {
            pm.set_active(h24 >= 12);
            let h12 = h24 % 12;
            spin.set_value(if h12 == 0 { 12.0 } else { h12 as f64 });
        } else {
            pm.set_active(false);
            spin.set_value(h24 as f64);
        }
    }

    /// Reshape every hour spinner + AM/PM toggle when the dialog-wide 12h
    /// switch flips. Preserves the underlying 24-hour value.
    fn apply_time_mode(&self, twelve: bool) {
        // Snapshot current 24h values first using the *old* mode.
        let pairs: [(gtk::SpinButton, gtk::ToggleButton); 4] = [
            (self.daily_hour.clone(), self.daily_pm.clone()),
            (self.weekly_hour.clone(), self.weekly_pm.clone()),
            (self.monthly_hour.clone(), self.monthly_pm.clone()),
            (self.one_time_hour.clone(), self.one_time_pm.clone()),
        ];
        let snapshot: Vec<u32> = pairs
            .iter()
            .map(|(spin, pm)| self.read_hour(spin, pm))
            .collect();
        // Flip mode, then reshape each spinner.
        self.twelve_h.set(twelve);
        for (spin, pm) in &pairs {
            if twelve {
                spin.set_range(1.0, 12.0);
                pm.set_visible(true);
            } else {
                pm.set_visible(false);
                pm.set_active(false);
                spin.set_range(0.0, 23.0);
            }
        }
        // Write the preserved 24h values back through the new mode.
        for ((spin, pm), h24) in pairs.iter().zip(snapshot.iter()) {
            self.set_hour_24(spin, pm, *h24);
        }
    }


    /// Reshape the preset list for the active backend + its capabilities.
    pub fn set_backend(&self, backend: Backend, caps: Capabilities) {
        let mut presets: Vec<Preset> = Vec::new();
        let is_cron = matches!(backend, Backend::CronUser | Backend::CronSystem);

        if caps.on_calendar {
            presets.push(Preset::Daily);
            presets.push(Preset::Weekly);
            presets.push(Preset::Monthly);
            presets.push(Preset::EveryN);
            if !is_cron {
                // Cron has no "one time" — would silently repeat every year.
                presets.push(Preset::OneTime);
            }
        }
        if caps.on_boot {
            presets.push(Preset::AtBoot);
        }
        if caps.on_login {
            presets.push(Preset::AtLogin);
        }
        if caps.on_device {
            presets.push(Preset::OnDevice);
        }
        if caps.on_calendar || caps.on_boot {
            presets.push(Preset::Advanced);
        }

        // Rebuild the StringList model.
        while self.preset_model.n_items() > 0 {
            self.preset_model.remove(0);
        }
        for p in &presets {
            self.preset_model.append(p.label());
        }

        self.inner.borrow_mut().backend = backend;
        self.inner.borrow_mut().caps = caps;
        self.inner.borrow_mut().presets = presets.clone();

        if !presets.is_empty() {
            self.preset_dropdown.set_selected(0);
            self.stack.set_visible_child_name(presets[0].stack_name());
        }

        // Single-preset backends (Autostart, OpenRC) — disable the dropdown.
        self.preset_dropdown.set_sensitive(presets.len() > 1);
    }

    /// Compose the final `Trigger`. Returns Err with a user-facing message
    /// when the input is invalid (e.g. no weekdays selected).
    pub fn trigger(&self) -> Result<Trigger, String> {
        let inner = self.inner.borrow();
        let preset = inner
            .presets
            .get(self.preset_dropdown.selected() as usize)
            .copied()
            .ok_or_else(|| "No schedule preset available".to_string())?;
        let is_cron = matches!(inner.backend, Backend::CronUser | Backend::CronSystem);

        match preset {
            Preset::Daily => {
                let h = self.read_hour(&self.daily_hour, &self.daily_pm);
                let m = self.daily_minute.value() as u32;
                Ok(Trigger::OnCalendar(if is_cron {
                    format!("cron:{m} {h} * * *")
                } else {
                    format!("*-*-* {h:02}:{m:02}:00")
                }))
            }
            Preset::Weekly => {
                let days: Vec<usize> = (0..7).filter(|i| self.weekly_toggles[*i].is_active()).collect();
                if days.is_empty() {
                    return Err("Pick at least one weekday".into());
                }
                let h = self.read_hour(&self.weekly_hour, &self.weekly_pm);
                let m = self.weekly_minute.value() as u32;
                if is_cron {
                    // cron DOW: Sun=0..Sat=6. Our toggles: Mon=0..Sun=6.
                    let dow_map = [1, 2, 3, 4, 5, 6, 0];
                    let dows: Vec<String> =
                        days.iter().map(|i| dow_map[*i].to_string()).collect();
                    Ok(Trigger::OnCalendar(format!(
                        "cron:{m} {h} * * {}",
                        dows.join(",")
                    )))
                } else {
                    let names: Vec<&str> = days.iter().map(|i| WEEKDAYS[*i]).collect();
                    Ok(Trigger::OnCalendar(format!(
                        "{} *-*-* {h:02}:{m:02}:00",
                        names.join(",")
                    )))
                }
            }
            Preset::Monthly => {
                let d = self.monthly_day.value() as u32;
                let h = self.read_hour(&self.monthly_hour, &self.monthly_pm);
                let m = self.monthly_minute.value() as u32;
                Ok(Trigger::OnCalendar(if is_cron {
                    format!("cron:{m} {h} {d} * *")
                } else {
                    format!("*-*-{d:02} {h:02}:{m:02}:00")
                }))
            }
            Preset::EveryN => {
                let n = self.every_n_value.value().max(1.0) as u32;
                let unit_hours = self.every_n_unit.selected() == 1;
                Ok(Trigger::OnCalendar(if is_cron {
                    if unit_hours {
                        format!("cron:0 */{n} * * *")
                    } else {
                        format!("cron:*/{n} * * * *")
                    }
                } else if unit_hours {
                    format!("*-*-* 0/{n}:00:00")
                } else {
                    format!("*:0/{n}")
                }))
            }
            Preset::OneTime => {
                // gtk::Calendar reports month 0–11; OnCalendar needs 1–12.
                let date = self.one_time_calendar.date();
                let y = date.year();
                let mo = date.month();
                let d = date.day_of_month();
                let h = self.read_hour(&self.one_time_hour, &self.one_time_pm);
                let mi = self.one_time_minute.value() as u32;
                Ok(Trigger::OnCalendar(format!(
                    "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:00"
                )))
            }
            Preset::AtBoot => {
                let n = self.at_boot_value.value().max(0.0) as u32;
                let unit = if self.at_boot_unit.selected() == 0 { "s" } else { "min" };
                Ok(Trigger::OnBootSec(format!("{n}{unit}")))
            }
            Preset::AtLogin => Ok(Trigger::OnLogin),
            Preset::OnDevice => {
                let action = match self.on_device_event.selected() {
                    1 => DeviceAction::Remove,
                    2 => DeviceAction::Change,
                    _ => DeviceAction::Add,
                };
                let subsystem = self.on_device_subsystem.text().trim().to_lowercase();
                if subsystem.is_empty() {
                    return Err(
                        "Pick a device or enter a subsystem in the custom match form".into(),
                    );
                }
                let vendor_id = nonempty_lower(self.on_device_vendor.text().as_str());
                let product_id = nonempty_lower(self.on_device_product.text().as_str());
                let kernel = nonempty(self.on_device_kernel.text().as_str());
                if vendor_id.is_none() && product_id.is_none() && kernel.is_none() {
                    return Err(
                        "Add a vendor:product pair or a kernel name pattern so the rule matches one device"
                            .into(),
                    );
                }
                let label = self.on_device_label.borrow().clone();
                Ok(Trigger::OnDevice(DeviceMatch {
                    action,
                    subsystem,
                    vendor_id,
                    product_id,
                    kernel,
                    attrs: Vec::new(),
                    label,
                }))
            }
            Preset::Advanced => {
                let raw = self.advanced_entry.text().trim().to_string();
                if raw.is_empty() {
                    return Err("Type a schedule expression, or pick a preset".into());
                }
                // We do not know if user meant boot or calendar — assume calendar.
                // OnBootSec via advanced is rare; AtBoot preset covers the case.
                Ok(Trigger::OnCalendar(raw))
            }
        }
    }

    /// Wire the "Pick device…" button to the device picker. The dialog owns
    /// the parent window, so it has to install the click handler — we just
    /// expose this convenience for it.
    pub fn bind_device_picker(&self, parent: adw::ApplicationWindow) {
        let summary = self.on_device_summary.clone();
        let subsystem = self.on_device_subsystem.clone();
        let vendor = self.on_device_vendor.clone();
        let product = self.on_device_product.clone();
        let kernel = self.on_device_kernel.clone();
        let label_slot = self.on_device_label.clone();
        self.on_device_pick_btn.connect_clicked(move |_| {
            let summary = summary.clone();
            let subsystem = subsystem.clone();
            let vendor = vendor.clone();
            let product = product.clone();
            let kernel = kernel.clone();
            let label_slot = label_slot.clone();
            present_device_picker(&parent, move |d| {
                subsystem.set_text(&d.subsystem);
                vendor.set_text(d.vendor_id.as_deref().unwrap_or(""));
                product.set_text(d.product_id.as_deref().unwrap_or(""));
                kernel.set_text(d.kernel.as_deref().unwrap_or(""));
                let label = d.label();
                summary.set_text(&label);
                *label_slot.borrow_mut() = Some(label);
            });
        });
    }

    /// Populate widgets from an existing `Trigger`. Picks the best preset
    /// match; anything that doesn't fit a structured preset lands on the
    /// Advanced page with its raw expression preserved.
    pub fn load(&self, trigger: &Trigger) {
        let inner = self.inner.borrow();
        let presets = inner.presets.clone();
        drop(inner);
        let select = |p: Preset| {
            if let Some(idx) = presets.iter().position(|x| *x == p) {
                self.preset_dropdown.set_selected(idx as u32);
                self.stack.set_visible_child_name(p.stack_name());
            }
        };
        match trigger {
            Trigger::OnLogin => select(Preset::AtLogin),
            Trigger::OnBootSec(v) => {
                select(Preset::AtBoot);
                // "5min" / "30s" → unit + magnitude
                let v = v.trim();
                if let Some(rest) = v.strip_suffix("min") {
                    if let Ok(n) = rest.trim().parse::<f64>() {
                        self.at_boot_value.set_value(n);
                        self.at_boot_unit.set_selected(1);
                    }
                } else if let Some(rest) = v.strip_suffix('s') {
                    if let Ok(n) = rest.trim().parse::<f64>() {
                        self.at_boot_value.set_value(n);
                        self.at_boot_unit.set_selected(0);
                    }
                }
            }
            Trigger::OnDevice(m) => {
                select(Preset::OnDevice);
                self.on_device_subsystem.set_text(&m.subsystem);
                self.on_device_vendor.set_text(m.vendor_id.as_deref().unwrap_or(""));
                self.on_device_product.set_text(m.product_id.as_deref().unwrap_or(""));
                self.on_device_kernel.set_text(m.kernel.as_deref().unwrap_or(""));
                self.on_device_event.set_selected(match m.action {
                    task_scheduler_core::DeviceAction::Add => 0,
                    task_scheduler_core::DeviceAction::Remove => 1,
                    task_scheduler_core::DeviceAction::Change => 2,
                });
                if let Some(lbl) = &m.label {
                    self.on_device_summary.set_text(lbl);
                    *self.on_device_label.borrow_mut() = Some(lbl.clone());
                }
            }
            Trigger::OnCalendar(expr) => {
                // Try to recognize our own preset shapes; otherwise fall back
                // to Advanced with the raw expression.
                if !try_load_calendar(self, expr, &presets) {
                    select(Preset::Advanced);
                    self.advanced_entry.set_text(expr);
                }
            }
        }
    }
}

fn try_load_calendar(s: &ScheduleBuilder, expr: &str, presets: &[Preset]) -> bool {
    let e = expr.trim();
    let select = |p: Preset| -> bool {
        if let Some(idx) = presets.iter().position(|x| *x == p) {
            s.preset_dropdown.set_selected(idx as u32);
            s.stack.set_visible_child_name(p.stack_name());
            true
        } else { false }
    };
    // cron:M H D M W
    if let Some(cron) = e.strip_prefix("cron:") {
        let parts: Vec<&str> = cron.split_whitespace().collect();
        if parts.len() == 5 {
            let (m, h, d, mo, w) = (parts[0], parts[1], parts[2], parts[3], parts[4]);
            // EveryN minute: */N * * * *
            if let Some(n) = m.strip_prefix("*/") {
                if h == "*" && d == "*" && mo == "*" && w == "*" {
                    if let Ok(n) = n.parse::<f64>() {
                        if select(Preset::EveryN) {
                            s.every_n_value.set_value(n);
                            s.every_n_unit.set_selected(0);
                            return true;
                        }
                    }
                }
            }
            // EveryN hour: 0 */N * * *
            if m == "0" && d == "*" && mo == "*" && w == "*" {
                if let Some(n) = h.strip_prefix("*/") {
                    if let Ok(n) = n.parse::<f64>() {
                        if select(Preset::EveryN) {
                            s.every_n_value.set_value(n);
                            s.every_n_unit.set_selected(1);
                            return true;
                        }
                    }
                }
            }
            // Daily: M H * * *
            if d == "*" && mo == "*" && w == "*" {
                if let (Ok(mm), Ok(hh)) = (m.parse::<f64>(), h.parse::<f64>()) {
                    if select(Preset::Daily) {
                        s.set_hour_24(&s.daily_hour, &s.daily_pm, hh as u32);
                        s.daily_minute.set_value(mm);
                        return true;
                    }
                }
            }
            // Monthly: M H D * *
            if mo == "*" && w == "*" {
                if let (Ok(mm), Ok(hh), Ok(dd)) =
                    (m.parse::<f64>(), h.parse::<f64>(), d.parse::<f64>())
                {
                    if select(Preset::Monthly) {
                        s.set_hour_24(&s.monthly_hour, &s.monthly_pm, hh as u32);
                        s.monthly_minute.set_value(mm);
                        s.monthly_day.set_value(dd);
                        return true;
                    }
                }
            }
            // Weekly: M H * * DOW(s)
            if d == "*" && mo == "*" && w != "*" {
                if let (Ok(mm), Ok(hh)) = (m.parse::<f64>(), h.parse::<f64>()) {
                    if select(Preset::Weekly) {
                        s.set_hour_24(&s.weekly_hour, &s.weekly_pm, hh as u32);
                        s.weekly_minute.set_value(mm);
                        // cron DOW 0=Sun..6=Sat -> toggle index 0=Mon..6=Sun
                        let dow_map = [6usize, 0, 1, 2, 3, 4, 5];
                        for tog in &s.weekly_toggles { tog.set_active(false); }
                        for token in w.split(',') {
                            if let Ok(n) = token.trim().parse::<usize>() {
                                if n < 7 { s.weekly_toggles[dow_map[n]].set_active(true); }
                            }
                        }
                        return true;
                    }
                }
            }
        }
        return false;
    }
    // systemd: Mon,Tue *-*-* HH:MM:SS  or  *-*-DD HH:MM:SS  or  *-*-* HH:MM:SS
    let parts: Vec<&str> = e.split_whitespace().collect();
    if parts.len() == 2 {
        let date = parts[0];
        let time = parts[1];
        let tparts: Vec<&str> = time.split(':').collect();
        if tparts.len() >= 2 {
            let hh = tparts[0].parse::<f64>().ok();
            let mm = tparts[1].parse::<f64>().ok();
            if let (Some(h), Some(m)) = (hh, mm) {
                if date == "*-*-*" {
                    if select(Preset::Daily) {
                        s.set_hour_24(&s.daily_hour, &s.daily_pm, h as u32);
                        s.daily_minute.set_value(m);
                        return true;
                    }
                }
                if let Some(rest) = date.strip_prefix("*-*-") {
                    if let Ok(d) = rest.parse::<f64>() {
                        if select(Preset::Monthly) {
                            s.monthly_day.set_value(d);
                            s.set_hour_24(&s.monthly_hour, &s.monthly_pm, h as u32);
                            s.monthly_minute.set_value(m);
                            return true;
                        }
                    }
                }
                // Weekday list
                if date.split(',').all(|d| WEEKDAYS.contains(&d)) {
                    if select(Preset::Weekly) {
                        s.set_hour_24(&s.weekly_hour, &s.weekly_pm, h as u32);
                        s.weekly_minute.set_value(m);
                        for tog in &s.weekly_toggles { tog.set_active(false); }
                        for d in date.split(',') {
                            if let Some(i) = WEEKDAYS.iter().position(|w| *w == d) {
                                s.weekly_toggles[i].set_active(true);
                            }
                        }
                        return true;
                    }
                }
            }
        }
    }
    false
}


fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}
fn nonempty_lower(s: &str) -> Option<String> {
    nonempty(s).map(|v| v.to_lowercase())
}

impl Default for ScheduleBuilder {
    fn default() -> Self { Self::new() }
}

/// Compact horizontal time picker: `[label]  [hh] : [mm]  [AM|PM]`.
/// The AM/PM toggle is hidden until the dialog-wide 12h switch is on.
fn time_row(label: &str) -> (gtk::Box, gtk::SpinButton, gtk::SpinButton, gtk::ToggleButton) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Start);
    row.set_valign(gtk::Align::Center);
    let lbl = gtk::Label::new(Some(label));
    let hour = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
    hour.set_wrap(true);
    hour.set_width_chars(2);
    hour.set_valign(gtk::Align::Center);
    hour.set_vexpand(false);
    hour.set_value(9.0);
    hour.connect_output(|spin| {
        spin.set_text(&format!("{:02}", spin.value() as i32));
        gtk::glib::Propagation::Stop
    });
    let sep = gtk::Label::new(Some(":"));
    let minute = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    minute.set_wrap(true);
    minute.set_width_chars(2);
    minute.set_valign(gtk::Align::Center);
    minute.set_vexpand(false);
    minute.set_value(0.0);
    minute.connect_output(|spin| {
        spin.set_text(&format!("{:02}", spin.value() as i32));
        gtk::glib::Propagation::Stop
    });
    let am_pm = gtk::ToggleButton::with_label("PM");
    am_pm.set_valign(gtk::Align::Center);
    am_pm.set_visible(false);
    row.append(&lbl);
    row.append(&hour);
    row.append(&sep);
    row.append(&minute);
    row.append(&am_pm);
    (row, hour, minute, am_pm)
}

fn labeled_spin(label: &str, min: f64, max: f64, step: f64) -> (gtk::Box, gtk::SpinButton) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Start);
    row.set_valign(gtk::Align::Center);
    let lbl = gtk::Label::new(Some(label));
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_value(min);
    spin.set_valign(gtk::Align::Center);
    spin.set_vexpand(false);
    row.append(&lbl);
    row.append(&spin);
    (row, spin)
}

