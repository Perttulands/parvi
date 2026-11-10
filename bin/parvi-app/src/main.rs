//! Parvi's desktop app. Each area's page is listed here, and nowhere else.

#![cfg_attr(windows, windows_subsystem = "windows")]

use parvi_app_kit::Build;

const PAGES: &[Build] = &[parvi_terminals_app::build, parvi_scheduled_app::build];

fn main() {
    parvi_app::run(PAGES);
}
