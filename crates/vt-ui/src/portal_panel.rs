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
    AppLaunch(String),
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
            Self::Url => Color32::from_rgb(114, 159, 207),
            Self::FilePath => Color32::from_rgb(138, 226, 52),
            Self::BuildOutput => Color32::from_rgb(252, 233, 79),
            Self::AppLaunch(_) => Color32::from_rgb(173, 127, 168),
        }
    }
}

/// A tracked running process launched from the terminal.
#[derive(Debug, Clone)]
pub struct TrackedProcess {
    pub name: String,
    pub command: String,
    pub output_lines: Vec<(String, OutputKind)>, // (line, kind)
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputKind {
    Stdout,
    Stderr,
    Info,
}

pub enum PortalAction {
    OpenUrl(String),
    OpenFile(String),
    Close,
    ToggleCollapse,
    ClearItems,
    EmbedByName(String),
    EmbedByPid(u32),
    ReleaseEmbed,
    GrabWindow,
}

pub struct PortalPanelResult {
    pub action: Option<PortalAction>,
    pub panel_width: f32,
}

/// Whitelisted applications to detect in terminal output.
pub const WHITELISTED_APPS: &[(&str, &str)] = &[
    ("code", "VS Code"),
    ("cursor", "Cursor"),
    ("vim", "Vim"),
    ("nvim", "Neovim"),
    ("nano", "Nano"),
    ("firefox", "Firefox"),
    ("chromium", "Chromium"),
    ("google-chrome", "Chrome"),
    ("brave", "Brave"),
    ("brave-browser", "Brave"),
    ("cargo", "Cargo"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("yarn", "Yarn"),
    ("python", "Python"),
    ("node", "Node.js"),
    ("docker", "Docker"),
    ("git", "Git"),
    ("make", "Make"),
    ("cmake", "CMake"),
    ("go", "Go"),
    ("rustc", "Rust"),
    ("unity", "Unity"),
    ("godot", "Godot"),
    ("blender", "Blender"),
];

/// Scan terminal output text for detectable items.
pub fn scan_output(text: &str) -> Vec<DetectedItem> {
    let mut items = Vec::new();
    let now = std::time::Instant::now();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Detect URLs
        for word in trimmed.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '/' && c != '.' && c != '-' && c != '_');
            if (clean.starts_with("http://") || clean.starts_with("https://"))
                && clean.len() > 10
            {
                items.push(DetectedItem {
                    kind: DetectedKind::Url,
                    value: clean.to_string(),
                    timestamp: now,
                });
            }
        }

        // Detect localhost:PORT
        if let Some(pos) = trimmed.find("localhost:") {
            let rest = &trimmed[pos..];
            let token = rest.split_whitespace().next().unwrap_or(rest);
            let url = format!("http://{}", token.trim_end_matches(|c: char| !c.is_alphanumeric() && c != ':'));
            if url.contains(':') && url.len() > 17 {
                items.push(DetectedItem {
                    kind: DetectedKind::Url,
                    value: url,
                    timestamp: now,
                });
            }
        }

        // Detect whitelisted app launches
        let cmd_part = if let Some(pos) = trimmed.find("$ ") {
            &trimmed[pos + 2..]
        } else {
            trimmed
        };
        let first_word = cmd_part.split_whitespace().next().unwrap_or("");
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

        // Detect build errors/warnings
        if trimmed.contains("error[E") || trimmed.contains("error:") {
            items.push(DetectedItem {
                kind: DetectedKind::BuildOutput,
                value: trimmed.chars().take(120).collect(),
                timestamp: now,
            });
        }
        if trimmed.contains("warning:") && !trimmed.contains("generated") {
            items.push(DetectedItem {
                kind: DetectedKind::BuildOutput,
                value: trimmed.chars().take(120).collect(),
                timestamp: now,
            });
        }

        // Detect file paths being modified/created
        if (trimmed.contains("Compiling ") || trimmed.contains("Creating ") || trimmed.contains("Writing "))
            && trimmed.contains('/')
        {
            items.push(DetectedItem {
                kind: DetectedKind::FilePath,
                value: trimmed.to_string(),
                timestamp: now,
            });
        }
    }

    items
}

/// Draw the portal side panel on the right.
pub fn draw_portal_panel(
    ctx: &egui::Context,
    detected_items: &[DetectedItem],
    collapsed: bool,
    has_embedded: bool,
) -> PortalPanelResult {
    let mut action = None;

    let panel_frame = egui::Frame::new()
        .fill(Color32::from_rgb(30, 30, 32))
        .inner_margin(egui::Margin::same(8));

    if collapsed {
        let panel_response = egui::SidePanel::right("portal_panel")
            .resizable(false)
            .exact_width(24.0)
            .frame(panel_frame)
            .show(ctx, |ui| {
                if ui.add(egui::Button::new(
                    RichText::new("<").color(Color32::from_rgb(150, 150, 150))
                ).small()).on_hover_text("Open portal").clicked() {
                    action = Some(PortalAction::ToggleCollapse);
                }
                // Show item count badge
                if !detected_items.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("{}", detected_items.len()))
                            .size(10.0)
                            .color(Color32::from_rgb(66, 133, 244)),
                    );
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
                ui.label(
                    RichText::new(format!("({})", detected_items.len()))
                        .size(11.0)
                        .color(Color32::from_rgb(100, 100, 100)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(">").on_hover_text("Collapse").clicked() {
                        action = Some(PortalAction::ToggleCollapse);
                    }
                    if has_embedded {
                        if ui.add(egui::Button::new(
                            RichText::new("Release").color(Color32::from_rgb(239, 41, 41)).size(11.0)
                        ).small()).on_hover_text("Release embedded window").clicked() {
                            action = Some(PortalAction::ReleaseEmbed);
                        }
                    }
                    if !has_embedded {
                        if ui.add(egui::Button::new(
                            RichText::new("Grab").color(Color32::from_rgb(66, 133, 244)).size(11.0)
                        ).small()).on_hover_text("Grab any running window into portal").clicked() {
                            action = Some(PortalAction::GrabWindow);
                        }
                    }
                    if !detected_items.is_empty() && !has_embedded {
                        if ui.small_button("Clear").on_hover_text("Clear all").clicked() {
                            action = Some(PortalAction::ClearItems);
                        }
                    }
                });
            });
            ui.separator();

            if detected_items.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("No activity detected")
                            .color(Color32::from_rgb(100, 100, 100)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Run commands in the terminal.\nURLs, builds, and app launches\nwill appear here.")
                            .color(Color32::from_rgb(80, 80, 80))
                            .size(11.0),
                    );
                });
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Group by kind
                        let mut urls: Vec<&DetectedItem> = Vec::new();
                        let mut builds: Vec<&DetectedItem> = Vec::new();
                        let mut apps: Vec<&DetectedItem> = Vec::new();
                        let mut files: Vec<&DetectedItem> = Vec::new();

                        for item in detected_items.iter().rev() {
                            match &item.kind {
                                DetectedKind::Url => urls.push(item),
                                DetectedKind::BuildOutput => builds.push(item),
                                DetectedKind::AppLaunch(_) => apps.push(item),
                                DetectedKind::FilePath => files.push(item),
                            }
                        }

                        // URLs section
                        if !urls.is_empty() {
                            ui.label(RichText::new("URLs").size(11.0).strong().color(Color32::from_rgb(114, 159, 207)));
                            for item in &urls {
                                if ui.link(
                                    RichText::new(&item.value).size(12.0).color(Color32::from_rgb(114, 159, 207)),
                                ).clicked() {
                                    action = Some(PortalAction::OpenUrl(item.value.clone()));
                                }
                            }
                            ui.add_space(8.0);
                        }

                        // Apps section
                        if !apps.is_empty() {
                            ui.label(RichText::new("Applications").size(11.0).strong().color(Color32::from_rgb(173, 127, 168)));
                            for item in &apps {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(item.kind.label())
                                            .size(10.0)
                                            .color(item.kind.color())
                                            .strong(),
                                    );
                                    ui.label(
                                        RichText::new(&item.value)
                                            .size(11.0)
                                            .color(Color32::from_rgb(200, 200, 200)),
                                    );
                                    if !has_embedded {
                                        if ui.add(egui::Button::new(
                                            RichText::new("Embed").size(10.0).color(Color32::from_rgb(66, 133, 244))
                                        ).small()).on_hover_text("Embed this app's window into the portal").clicked() {
                                            action = Some(PortalAction::EmbedByName(item.kind.label().to_string()));
                                        }
                                    }
                                });
                            }
                            ui.add_space(8.0);
                        }

                        // Build output section
                        if !builds.is_empty() {
                            ui.label(RichText::new("Build Output").size(11.0).strong().color(Color32::from_rgb(252, 233, 79)));
                            for item in &builds {
                                let color = if item.value.contains("error") {
                                    Color32::from_rgb(239, 41, 41)
                                } else {
                                    Color32::from_rgb(252, 233, 79)
                                };
                                ui.label(
                                    RichText::new(&item.value)
                                        .size(11.0)
                                        .color(color)
                                        .monospace(),
                                );
                            }
                            ui.add_space(8.0);
                        }

                        // Files section
                        if !files.is_empty() {
                            ui.label(RichText::new("Files").size(11.0).strong().color(Color32::from_rgb(138, 226, 52)));
                            for item in &files {
                                ui.label(
                                    RichText::new(&item.value)
                                        .size(11.0)
                                        .color(Color32::from_rgb(138, 226, 52)),
                                );
                            }
                        }
                    });
            }
        });

    let w = panel_response.response.rect.width();
    PortalPanelResult { action, panel_width: w }
}
