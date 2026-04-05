use egui::{Color32, Visuals};
use vt_core::types::Theme;

pub struct ThemeColors {
    pub bg: Color32,
    pub fg: Color32,
    pub panel_bg: Color32,
    pub tab_active: Color32,
    pub tab_inactive: Color32,
    pub accent: Color32,
    pub border: Color32,
    pub terminal_bg: [f64; 4], // RGBA for wgpu clear color
}

impl ThemeColors {
    pub fn from_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                bg: Color32::from_rgb(30, 30, 30),
                fg: Color32::from_rgb(211, 215, 207),
                panel_bg: Color32::from_rgb(37, 37, 38),
                tab_active: Color32::from_rgb(45, 45, 48),
                tab_inactive: Color32::from_rgb(37, 37, 38),
                accent: Color32::from_rgb(66, 133, 244),
                border: Color32::from_rgb(60, 60, 60),
                terminal_bg: [0.118, 0.118, 0.118, 1.0],
            },
            Theme::Light => Self {
                bg: Color32::from_rgb(255, 255, 255),
                fg: Color32::from_rgb(30, 30, 30),
                panel_bg: Color32::from_rgb(243, 243, 243),
                tab_active: Color32::from_rgb(255, 255, 255),
                tab_inactive: Color32::from_rgb(236, 236, 236),
                accent: Color32::from_rgb(66, 133, 244),
                border: Color32::from_rgb(200, 200, 200),
                terminal_bg: [1.0, 1.0, 1.0, 1.0],
            },
        }
    }

    pub fn apply_to_egui(&self, ctx: &egui::Context, theme: Theme) {
        let visuals = match theme {
            Theme::Dark => Visuals::dark(),
            Theme::Light => Visuals::light(),
        };
        ctx.set_visuals(visuals);
    }
}
