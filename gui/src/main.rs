mod app;
mod error_log;
mod platform;
mod scheduler;
mod ui;

use adw::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "org.linux.TaskScheduler";

fn main() -> glib::ExitCode {
    let application = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    let desktop = platform::detect();

    application.connect_startup(move |_| {
        platform::theme::apply(desktop);
        platform::css::install(desktop);
    });

    application.connect_activate(move |app| app::build_ui(app, desktop));
    application.run()
}
