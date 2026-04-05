use egui::{self, Color32, RichText};

/// A detected app or resource from terminal output.
#[derive(Debug, Clone)]
pub struct DetectedItem {
    pub kind: DetectedKind,
    pub value: String,
    pub timestamp: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetectedKind {
    Url,
    FilePath,
    BuildOutput,
    AppLaunch(String), // app name
}

impl DetectedKind {
    pub fn label(&self) -> &str {
        match self {
            Self::Url => "URL",
            Self::FilePath => "File",
            Self::BuildOutput => "Build",
            Self::AppLaunch(name) => name.as_str(),
        }
    }

    pub fn color(&self) -> Color32 {
        match self {
            Self::Url => Color32::from_rgb(114, 159, 207),        // blue
            Self::FilePath => Color32::from_rgb(138, 226, 52),     // green
            Self::BuildOutput => Color32::from_rgb(252, 233, 79),  // yellow
            Self::AppLaunch(_) => Color32::from_rgb(173, 127, 168), // purple
        }
    }
}

pub enum PortalAction {
    OpenUrl(String),
    OpenFile(String),
    Close,
    ToggleCollapse,
}

pub struct PortalPanelResult {
    pub action: Option<PortalAction>,
    pub panel_width: f32,
}

/// Whitelisted applications to detect in terminal output.
const WHITELISTED_APPS: &[(&str, &str)] = &[
    ("code", "VS Code"),
    ("cursor", "Cursor"),
    ("vim", "Vim"),
    ("nvim", "Neovim"),
    ("nano", "Nano"),
    ("firefox", "Firefox"),
    ("chromium", "Chromium"),
    ("google-chrome", "Chrome"),
    ("cargo", "Cargo"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("yarn", "Yarn"),
    ("python", "Python"),
    ("node", "Node.js"),
    ("docker", "Docker"),
    ("git", "Git"),
];

/// Scan terminal output text for detectable items.
pub fn scan_output(text: &str) -> Vec<DetectedItem> {
    let mut items = Vec::new();
    let now = std::time::Instant::now();

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect URLs (http/https)
        for word in trimmed.split_whitespace() {
            if (word.starts_with("http://") || word.starts_with("https://"))
                && word.len() > 10
            {
                // Clean trailing punctuation
                let url = word.trim_end_matches(|c: char| {
                    matches!(c, ',' | '.' | ')' | ']' | '"' | '\'')
                });
                items.push(DetectedItem {
                    kind: DetectedKind::Url,
                    value: url.to_string(),
                    timestamp: now,
                });
            }
        }

        // Detect "localhost:PORT" pattern (dev servers)
        if let Some(pos) = trimmed.find("localhost:") {
            let rest = &trimmed[pos..];
            let url = if rest.starts_with("localhost:") {
                format!("http://{}", rest.split_whitespace().next().unwrap_or(rest))
            } else {
                continue;
            };
            if url.len() > 16 {
                items.push(DetectedItem {
                    kind: DetectedKind::Url,
                    value: url,
                    timestamp: now,
                });
            }
        }

        // Detect whitelisted app launches (command at start of line or after $)
        let cmd_part = if let Some(pos) = trimmed.find("$ ") {
            &trimmed[pos + 2..]
        } else {
            trimmed
        };
        let first_word = cmd_part.split_whitespace().next().unwrap_or("");
        // Strip path prefix
        let cmd_name = first_word.rsplit('/').next().unwrap_or(first_word);

        for (cmd, app_name) in WHITELISTED_APPS {
            if cmd_name == *cmd {
                items.push(DetectedItem {
                    kind: DetectedKind::AppLaunch(app_name.to_string()),
                    value: cmd_part.to_string(),
                    timestamp: now,
                });
                break;
            }
        }
    }

    items
}

/// Draw the portal side panel on the right.
pub fn draw_portal_panel(
    ctx: &egui::Context,
    detected_items: &[DetectedItem],
    collapsed: bool,
) -> PortalPanelResult {
    let mut action = None;

    let panel_frame = egui::Frame::new()
        .fill(Color32::from_rgb(30, 30, 32))
        .inner_margin(egui::Margin::same(8));

    if collapsed {
        let panel_response = egui::SidePanel::right("portal_panel")
            .resizable(false)
            .exact_width(32.0)
            .frame(panel_frame)
            .show(ctx, |ui| {
                if ui.button(">").on_hover_text("Open portal").clicked() {
                    action = Some(PortalAction::ToggleCollapse);
                }
            });
        let w = panel_response.response.rect.width();
        return PortalPanelResult { action, panel_width: w };
    }

    let panel_response = egui::SidePanel::right("portal_panel")
        .resizable(true)
        .default_width(300.0)
        .min_width(200.0)
        .max_width(600.0)
        .frame(panel_frame)
        .show(ctx, |ui| {
            // Header
            ui.horizontal(|ui| {
                ui.label(RichText::new("Portal").strong().color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("<").on_hover_text("Collapse portal").clicked() {
                        action = Some(PortalAction::ToggleCollapse);
                    }
                });
            });
            ui.separator();

            if detected_items.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("No activity detected")
                            .color(Color32::from_rgb(100, 100, 100))
                            .size(13.0),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Run commands in the terminal.\nURLs, builds, and app launches\nwill appear here.")
                            .color(Color32::from_rgb(80, 80, 80))
                            .size(11.0),
                    );
                });
            } else {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Show items grouped by kind, newest first
                    for item in detected_items.iter().rev() {
                        let color = item.kind.color();
                        let label = item.kind.label();

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(label)
                                    .color(color)
                                    .size(10.0)
                                    .strong(),
                            );

                            match &item.kind {
                                DetectedKind::Url => {
                                    if ui.link(
                                        RichText::new(&item.value)
                                            .color(Color32::from_rgb(114, 159, 207))
                                            .size(12.0),
                                    ).clicked() {
                                        action = Some(PortalAction::OpenUrl(item.value.clone()));
                                    }
                                }
                                DetectedKind::FilePath => {
                                    if ui.link(
                                        RichText::new(&item.value)
                                            .color(Color32::from_rgb(138, 226, 52))
                                            .size(12.0),
                                    ).clicked() {
                                        action = Some(PortalAction::OpenFile(item.value.clone()));
                                    }
                                }
                                _ => {
                                    ui.label(
                                        RichText::new(&item.value)
                                            .color(Color32::from_rgb(200, 200, 200))
                                            .size(12.0),
                                    );
                                }
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
            }
        });

    let w = panel_response.response.rect.width();
    PortalPanelResult { action, panel_width: w }
}
