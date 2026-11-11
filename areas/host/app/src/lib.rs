//! Host readings and services pages.
mod chart;
mod floor;
mod host_store;
mod readings;
mod services;
mod services_store;
mod tokens;

use gpui::{App, Window};
use parvi_app_kit::{Host, Mounted, mount};

pub fn build_host(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, readings::ReadingsPage::new)
}
pub fn build_services(host: Host, window: &mut Window, cx: &mut App) -> Mounted {
    mount(host, window, cx, services::ServicesPage::new)
}
