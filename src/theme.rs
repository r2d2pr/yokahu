use eframe::egui::{self, Color32, CornerRadius, Stroke};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub preset_name: String,
    pub is_light: bool,
    pub bg_root: [u8; 3],
    pub bg_sidebar: [u8; 3],
    pub bg_surface: [u8; 3],
    pub bg_elevated: [u8; 3],
    pub border: [u8; 3],
    pub text: [u8; 3],
    pub text_muted: [u8; 3],
    pub accent: [u8; 3],
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self::preset("Obsidian Crimson")
    }
}

impl ThemeConfig {
    pub fn presets() -> &'static [&'static str] {
        &[
            "Obsidian Crimson",
            "Midnight Blue",
            "Emerald Slate",
            "Apple Light",
            "Pure Black",
        ]
    }

    pub fn preset(name: &str) -> Self {
        match name {
            "Midnight Blue" => Self {
                preset_name: name.to_string(),
                is_light: false,
                bg_root: [11, 14, 20],
                bg_sidebar: [16, 20, 29],
                bg_surface: [21, 26, 38],
                bg_elevated: [29, 36, 52],
                border: [40, 49, 70],
                text: [232, 237, 245],
                text_muted: [140, 152, 172],
                accent: [88, 166, 255],
            },
            "Emerald Slate" => Self {
                preset_name: name.to_string(),
                is_light: false,
                bg_root: [11, 16, 15],
                bg_sidebar: [15, 22, 20],
                bg_surface: [21, 30, 27],
                bg_elevated: [29, 41, 37],
                border: [40, 58, 52],
                text: [230, 242, 238],
                text_muted: [138, 162, 153],
                accent: [63, 185, 120],
            },
            "Apple Light" => Self {
                preset_name: name.to_string(),
                is_light: true,
                bg_root: [245, 245, 247],
                bg_sidebar: [235, 236, 240],
                bg_surface: [255, 255, 255],
                bg_elevated: [240, 242, 246],
                border: [216, 219, 226],
                text: [29, 29, 31],
                text_muted: [110, 110, 115],
                accent: [0, 113, 227],
            },
            "Pure Black" => Self {
                preset_name: name.to_string(),
                is_light: false,
                bg_root: [0, 0, 0],
                bg_sidebar: [10, 10, 12],
                bg_surface: [18, 18, 22],
                bg_elevated: [28, 28, 34],
                border: [42, 42, 50],
                text: [242, 242, 247],
                text_muted: [142, 142, 147],
                accent: [255, 69, 58],
            },
            _ => Self {
                preset_name: "Obsidian Crimson".to_string(),
                is_light: false,
                bg_root: [11, 12, 16],
                bg_sidebar: [16, 18, 24],
                bg_surface: [22, 25, 34],
                bg_elevated: [31, 35, 47],
                border: [45, 50, 66],
                text: [236, 238, 244],
                text_muted: [145, 150, 166],
                accent: [230, 57, 70],
            },
        }
    }

    pub fn c_root(&self) -> Color32 { rgb(self.bg_root) }
    pub fn c_sidebar(&self) -> Color32 { rgb(self.bg_sidebar) }
    pub fn c_surface(&self) -> Color32 { rgb(self.bg_surface) }
    pub fn c_elevated(&self) -> Color32 { rgb(self.bg_elevated) }
    pub fn c_border(&self) -> Color32 { rgb(self.border) }
    pub fn c_text(&self) -> Color32 { rgb(self.text) }
    pub fn c_muted(&self) -> Color32 { rgb(self.text_muted) }
    pub fn c_accent(&self) -> Color32 { rgb(self.accent) }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn apply_theme(ctx: &egui::Context, t: &ThemeConfig) {
    let mut visuals = if t.is_light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = t.c_root();
    visuals.window_fill = t.c_surface();
    visuals.extreme_bg_color = t.c_root();
    visuals.widgets.noninteractive.bg_fill = t.c_surface();
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.c_border());
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.c_text());
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    visuals.widgets.inactive.bg_fill = t.c_elevated();
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, t.c_border());
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, t.c_text());
    visuals.widgets.inactive.corner_radius = CornerRadius::same(10);

    visuals.widgets.hovered.bg_fill = t.c_elevated();
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, t.c_accent());
    visuals.widgets.hovered.corner_radius = CornerRadius::same(10);

    visuals.widgets.active.bg_fill = t.c_elevated();
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, t.c_accent());
    visuals.widgets.active.corner_radius = CornerRadius::same(10);

    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(t.accent[0], t.accent[1], t.accent[2], 55);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_stroke = Stroke::new(1.0, t.c_border());
    ctx.set_visuals(visuals);
}
