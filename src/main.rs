// During incremental development some helpers (delete endpoints, parse_sal, etc.) are
// part of the library surface but not yet wired into the UI; silence dead-code noise.
#![allow(dead_code)]
// On Windows, don't pop a console window behind the GUI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod error;
mod frontends;
mod keychain;
mod model;
mod mudmobile;
mod sal;
mod sge;
mod worker;

use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::init();

    // MUD Mobile brand mark, rasterized from ../mudmobile/web/src/app/icon.svg.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("embedded icon.png is valid");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 580.0])
            .with_min_inner_size([440.0, 440.0])
            .with_title("MUD Mobile Connector")
            // Wayland uses the app id to match the window to its .desktop entry (icon,
            // name in the titlebar/overview); X11 uses it as the WM class.
            .with_app_id("com.mudmobile.connector")
            .with_icon(std::sync::Arc::new(icon)),
        ..Default::default()
    };

    eframe::run_native(
        "MUD Mobile Connector",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx)))),
    )
}
