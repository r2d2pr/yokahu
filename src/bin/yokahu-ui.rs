#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::sync::Arc;
use yokahu::ui::YokahuApp;

const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");

fn main() -> eframe::Result<()> {
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 780.0])
        .with_min_inner_size([860.0, 560.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        vp = vp.with_icon(Arc::new(icon));
    }
    let opts = eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    };
    eframe::run_native("Yokahu", opts, Box::new(|cc| Ok(Box::new(YokahuApp::new(cc)))))
}
