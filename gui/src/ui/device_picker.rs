//! Browse currently-attached devices via `udevadm info --export-db` and let
//! the user pick one to use as a hardware-attach trigger match.

use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;


#[derive(Clone, Debug)]
pub struct AttachedDevice {
    pub subsystem: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub kernel: Option<String>,
    pub vendor_name: Option<String>,
    pub model_name: Option<String>,
}

impl AttachedDevice {
    pub fn label(&self) -> String {
        let name = match (&self.vendor_name, &self.model_name) {
            (Some(v), Some(m)) => format!("{v} {m}"),
            (None, Some(m)) => m.clone(),
            (Some(v), None) => v.clone(),
            _ => self
                .kernel
                .clone()
                .unwrap_or_else(|| "(unnamed device)".into()),
        };
        let ids = match (&self.vendor_id, &self.product_id) {
            (Some(v), Some(p)) => format!(" — {v}:{p}"),
            _ => String::new(),
        };
        format!("{name}{ids}")
    }

}

thread_local! {
    static CACHE: RefCell<Option<Vec<AttachedDevice>>> = const { RefCell::new(None) };
}

pub fn scan_devices(force: bool) -> Result<Vec<AttachedDevice>, String> {
    if !force {
        if let Some(v) = CACHE.with(|c| c.borrow().clone()) {
            return Ok(v);
        }
    }
    let out = Command::new("udevadm")
        .args(["info", "--export-db"])
        .output()
        .map_err(|e| format!("udevadm not available: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "udevadm failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let devices = parse_export_db(&text);
    CACHE.with(|c| *c.borrow_mut() = Some(devices.clone()));
    Ok(devices)
}

fn parse_export_db(text: &str) -> Vec<AttachedDevice> {
    let mut devices = Vec::new();
    let mut subsystem: Option<String> = None;
    let mut vendor_id: Option<String> = None;
    let mut product_id: Option<String> = None;
    let mut kernel: Option<String> = None;
    let mut vendor_name: Option<String> = None;
    let mut model_name: Option<String> = None;

    let flush = |devices: &mut Vec<AttachedDevice>,
                 subsystem: &mut Option<String>,
                 vendor_id: &mut Option<String>,
                 product_id: &mut Option<String>,
                 kernel: &mut Option<String>,
                 vendor_name: &mut Option<String>,
                 model_name: &mut Option<String>| {
        if let Some(sub) = subsystem.take() {
            devices.push(AttachedDevice {
                subsystem: sub,
                vendor_id: vendor_id.take(),
                product_id: product_id.take(),
                kernel: kernel.take(),
                vendor_name: vendor_name.take(),
                model_name: model_name.take(),
            });
        } else {
            *vendor_id = None;
            *product_id = None;
            *kernel = None;
            *vendor_name = None;
            *model_name = None;
        }
    };

    for line in text.lines() {
        if line.is_empty() {
            flush(
                &mut devices,
                &mut subsystem,
                &mut vendor_id,
                &mut product_id,
                &mut kernel,
                &mut vendor_name,
                &mut model_name,
            );
            continue;
        }
        let Some((tag, rest)) = line.split_once(": ") else { continue };
        match tag {
            "E" => {
                let Some((k, v)) = rest.split_once('=') else { continue };
                match k {
                    "SUBSYSTEM" => subsystem = Some(v.to_string()),
                    "ID_VENDOR_ID" => vendor_id = Some(v.to_lowercase()),
                    "ID_MODEL_ID" => product_id = Some(v.to_lowercase()),
                    "ID_VENDOR_FROM_DATABASE" => vendor_name = Some(v.to_string()),
                    "ID_MODEL_FROM_DATABASE" => model_name = Some(v.to_string()),
                    "ID_VENDOR" if vendor_name.is_none() => {
                        vendor_name = Some(v.replace('_', " "))
                    }
                    "ID_MODEL" if model_name.is_none() => {
                        model_name = Some(v.replace('_', " "))
                    }
                    _ => {}
                }
            }
            "N" if kernel.is_none() => kernel = Some(rest.to_string()),
            _ => {}
        }
    }
    // Trailing record without blank line.
    flush(
        &mut devices,
        &mut subsystem,
        &mut vendor_id,
        &mut product_id,
        &mut kernel,
        &mut vendor_name,
        &mut model_name,
    );
    devices.retain(is_triggerable);

    // Several subsystems expose multiple kernel nodes per physical device.
    // A udev trigger matches on vendor/product, so clear the kernel field for
    // these subsystems so the sort+dedup below collapses them to one entry.
    for d in &mut devices {
        let has_id = d.vendor_id.is_some() || d.product_id.is_some();
        match d.subsystem.as_str() {
            // Each USB HID device gets event*, mouse*, js* nodes.
            "input" if has_id => d.kernel = None,
            // Each USB audio device gets snd/pcm*, snd/control*, snd/hw* nodes.
            "sound" if has_id => d.kernel = None,
            // Each USB camera gets video0, video1, … nodes.
            "video4linux" if has_id => d.kernel = None,
            // USB devices may appear on multiple hub ports.
            "usb" if has_id => d.kernel = None,
            _ => {}
        }
    }

    devices.sort_by(|a, b| {
        a.subsystem
            .cmp(&b.subsystem)
            .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
    });
    // Deduplicate by (subsystem, vendor, product, kernel).
    devices.dedup_by(|a, b| {
        a.subsystem == b.subsystem
            && a.vendor_id == b.vendor_id
            && a.product_id == b.product_id
            && a.kernel == b.kernel
    });
    devices
}

/// Return true only for devices worth offering as hotplug trigger targets.
///
/// Filters out:
/// - Subsystems that represent internal / non-hotpluggable hardware
/// - Block device partitions and virtual block nodes (dm-*, loop*, ram*, zram*)
/// - Entries with no matchable identity at all
fn is_triggerable(d: &AttachedDevice) -> bool {
    // Must have at least one field we can write a udev match rule against.
    if d.vendor_id.is_none() && d.product_id.is_none() && d.kernel.is_none() {
        return false;
    }
    // Subsystems that are internal/virtual and never produce hotplug events,
    // or whose entries are sub-nodes already covered by another subsystem.
    match d.subsystem.as_str() {
        // ── Always-internal hardware ─────────────────────────────────────────
        | "platform" | "pci" | "pci_bus" | "pcie_port_service"
        | "acpi" | "acpi_pad"
        | "cpu" | "cpuid" | "cpuidle" | "cpufreq"
        | "memory" | "node"
        | "thermal" | "thermal_cooling_device"
        | "backlight" | "leds"
        | "firmware" | "dmi"
        | "clockevents" | "clocksource"
        | "workqueue" | "bsg"
        | "scsi_host" | "scsi_disk" | "scsi_generic"
        | "serio" | "serio_raw"
        | "i2c_adapter" | "i2c-dev" | "i2c"
        | "spi_master" | "spi"
        | "edac" | "pps" | "ptp"
        | "iio" | "iio_device"
        | "rapidio" | "mmc_host"
        | "dma_heap"
        | "drm" | "drm_dp_aux_dev"   // internal GPU; USB displays show under "usb"
        | "graphics"                   // framebuffer devices
        | "usb_power_delivery"
        | "wmi"
        | "gpio" | "mei"
        | "rfkill" | "rtc"
        | "tpm" | "tpmrm"
        | "msr"                        // CPU model-specific registers (one per core)
        | "mtd"                        // flash/NVRAM chips
        // ── Virtual character devices ────────────────────────────────────────
        | "mem"                        // /dev/null, /dev/random, etc.
        | "vc"                         // virtual consoles (vcs*, vcsa*)
        // ── Sub-nodes that duplicate a parent already in another subsystem ───
        | "hidraw"                     // raw HID nodes; covered by "hid" / "usb"
        | "usbmisc"                    // hiddev*, lp*; covered by "usb"
        | "media"                      // media nodes; covered by "video4linux"
        | "nvme" | "nvme-generic"      // controller/char nodes; covered by "block"
        | "misc" => return false,
        _ => {}
    }
    // Entries with no identity at all after subsystem check.
    if d.vendor_id.is_none() && d.product_id.is_none() {
        // tty: keep only real adapters (ttyUSB*, ttyACM*) — they have vendor/product.
        //      Drop tty0–63, ttyS0–31, console, ptmx which are all virtual.
        // sound: keep only real sound cards — drop snd/pcm*, snd/control*, etc.
        // (Both filters collapse to: drop if no identity.)
        match d.subsystem.as_str() {
            "tty" | "sound" | "input" => return false,
            _ => {}
        }
    }
    // USB root hubs (Linux Foundation vid=1d6b) are virtual, not real devices.
    if d.subsystem == "usb" {
        if d.vendor_id.as_deref() == Some("1d6b") {
            return false;
        }
    }
    // Block subsystem: keep whole disks, drop partitions and virtual nodes.
    if d.subsystem == "block" {
        if let Some(k) = &d.kernel {
            // Device-mapper (dm-0, dm-1, …)
            if k.starts_with("dm-") { return false; }
            // Loop devices
            if k.starts_with("loop") { return false; }
            // RAM / ZRAM
            if k.starts_with("ram") || k.starts_with("zram") { return false; }
            // SATA/SCSI partitions: sda1, sdb3, …  (letter(s) then digits)
            if k.starts_with("sd") && k.len() > 3 {
                let after_letter: &str = k.trim_start_matches(|c: char| c.is_ascii_alphabetic());
                if !after_letter.is_empty() && after_letter.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
            }
            // NVMe partitions: nvme0n1p1, nvme1n2p3, …
            if k.starts_with("nvme") {
                if let Some(p) = k.rfind('p') {
                    let suffix = &k[p + 1..];
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        return false;
                    }
                }
            }
            // eMMC partitions: mmcblk0p1, mmcblk1p2, …
            if k.starts_with("mmcblk") {
                if let Some(p) = k.rfind('p') {
                    let suffix = &k[p + 1..];
                    if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                        return false;
                    }
                }
            }
            // MD / software RAID: md0, md127, …
            if k.starts_with("md") && k[2..].chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
    }
    true
}

pub fn present_device_picker(
    parent: &adw::ApplicationWindow,
    on_pick: impl Fn(AttachedDevice) + 'static,
) {
    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Choose Device")
        .default_width(560)
        .default_height(640)
        .build();

    let header = adw::HeaderBar::new();
    crate::platform::style_header(&header, crate::platform::current());
    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Rescan attached devices"));
    header.pack_end(&refresh_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search by subsystem, vendor, model…")
        .build();
    body.append(&search);

    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(vec!["boxed-list".to_string()])
        .build();
    scroller.set_child(Some(&list_box));
    body.append(&scroller);

    let status = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .css_classes(vec!["dim-label".to_string()])
        .build();
    body.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(6);
    let cancel_btn = gtk::Button::with_label("Cancel");
    let select_btn = gtk::Button::builder()
        .label("Use this device")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    select_btn.set_sensitive(false);
    actions.append(&cancel_btn);
    actions.append(&select_btn);
    body.append(&actions);

    toolbar.set_content(Some(&body));
    window.set_content(Some(&toolbar));

    let devices: Rc<RefCell<Vec<AttachedDevice>>> = Rc::new(RefCell::new(Vec::new()));
    let reload = {
        let devices = devices.clone();
        let list_box = list_box.clone();
        let status = status.clone();
        move |force: bool| match scan_devices(force) {
            Ok(d) => {
                status.set_text(&format!("{} devices", d.len()));
                *devices.borrow_mut() = d;
                populate_rows(&list_box, &devices.borrow());
            }
            Err(e) => {
                status.set_text(&format!("Scan failed: {e}"));
            }
        }
    };
    reload(false);

    {
        let devices = devices.clone();
        let search = search.clone();
        list_box.set_filter_func(move |row| {
            let text = search.text().to_string().to_lowercase();
            if text.is_empty() {
                return true;
            }
            let idx = row.index() as usize;
            let devices = devices.borrow();
            let Some(d) = devices.get(idx) else { return false };
            d.subsystem.to_lowercase().contains(&text)
                || d.label().to_lowercase().contains(&text)
                || d.vendor_id.as_deref().unwrap_or("").contains(&text)
                || d.product_id.as_deref().unwrap_or("").contains(&text)
        });
    }
    {
        let list_box = list_box.clone();
        search.connect_search_changed(move |_| list_box.invalidate_filter());
    }
    {
        let select_btn = select_btn.clone();
        list_box.connect_row_selected(move |_, row| select_btn.set_sensitive(row.is_some()));
    }

    let on_pick: Rc<dyn Fn(AttachedDevice)> = Rc::new(on_pick);
    let commit = {
        let window = window.clone();
        let devices = devices.clone();
        let list_box = list_box.clone();
        let on_pick = on_pick.clone();
        move || {
            if let Some(row) = list_box.selected_row() {
                let idx = row.index() as usize;
                if let Some(d) = devices.borrow().get(idx).cloned() {
                    on_pick(d);
                    window.close();
                }
            }
        }
    };
    {
        let commit = commit.clone();
        list_box.connect_row_activated(move |_, _| commit());
    }
    {
        let commit = commit.clone();
        select_btn.connect_clicked(move |_| commit());
    }
    {
        let window = window.clone();
        cancel_btn.connect_clicked(move |_| window.close());
    }
    refresh_btn.connect_clicked(move |_| reload(true));

    window.present();
}

fn populate_rows(list_box: &gtk::ListBox, devices: &[AttachedDevice]) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    for d in devices {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&d.label()).as_str())
            .build();
        let mut subtitle = format!("subsystem: {}", d.subsystem);
        if let Some(k) = &d.kernel {
            subtitle.push_str(&format!(" · /dev/{k}"));
        }
        row.set_subtitle(glib::markup_escape_text(&subtitle).as_str());
        let icon = gtk::Image::from_icon_name(icon_for(&d.subsystem));
        icon.set_pixel_size(28);
        row.add_prefix(&icon);
        row.set_activatable(true);
        list_box.append(&row);
    }
}

fn icon_for(subsystem: &str) -> &'static str {
    match subsystem {
        "usb" => "drive-removable-media-usb-symbolic",
        "block" => "drive-harddisk-symbolic",
        "input" => "input-keyboard-symbolic",
        "net" => "network-wired-symbolic",
        "sound" => "audio-card-symbolic",
        "video4linux" | "drm" => "camera-web-symbolic",
        "bluetooth" => "bluetooth-symbolic",
        _ => "preferences-system-symbolic",
    }
}
