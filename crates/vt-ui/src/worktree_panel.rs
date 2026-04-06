use egui::{self, Color32, RichText};
use vt_core::types::Worktree;

/// Codicon icon constants (from VS Code's codicon.ttf)
pub mod icons {
    pub const SYNC: &str = "\u{EA77}";        // repo-sync
    pub const REPO_PULL: &str = "\u{EB40}";   // repo-pull
    pub const ADD: &str = "\u{EA60}";          // plus
    pub const CLOSE: &str = "\u{EA76}";        // x
    pub const CHEVRON_LEFT: &str = "\u{EAB6}"; // collapse left
    pub const CHEVRON_RIGHT: &str = "\u{EAB7}";// expand right
    pub const GIT_BRANCH: &str = "\u{EA68}";   // git branch
    pub const FOLDER: &str = "\u{EA83}";       // folder
    pub const FILE: &str = "\u{EA7B}";         // file
    pub const TERMINAL: &str = "\u{EA85}";     // terminal
    pub const SETTINGS: &str = "\u{EB52}";     // gear
    pub const SEARCH: &str = "\u{EA6D}";       // search
    pub const TRASH: &str = "\u{EA81}";        // delete
}

/// Actions emitted by the worktree panel.
pub enum WorktreeAction {
    Select(usize),
    CreateNew,
    Delete(usize),
    Refresh,
    PullRemote,
    ToggleCollapse,
    ResizeSidebar(f32),
}

/// Result from drawing the worktree panel.
pub struct WorktreePanelResult {
    pub action: Option<WorktreeAction>,
    pub panel_width: f32,
}

/// Draw the worktree sidebar panel. Returns action and actual panel width.
pub fn draw_worktree_panel(
    ctx: &egui::Context,
    worktrees: &[Worktree],
    selected_idx: Option<usize>,
    project_name: &str,
    has_remote_updates: bool,
    collapsed: bool,
    sidebar_width: f32,
) -> WorktreePanelResult {
    let mut action = None;

    let panel_frame = egui::Frame::new()
        .fill(Color32::from_rgb(37, 37, 38))
        .inner_margin(egui::Margin::same(8));

    if collapsed {
        // Collapsed: thin panel with just expand button
        let panel_response = egui::SidePanel::left("worktree_panel")
            .resizable(false)
            .exact_width(32.0)
            .frame(panel_frame)
            .show(ctx, |ui| {
                if ui.button(icons::CHEVRON_RIGHT).on_hover_text("Expand sidebar").clicked() {
                    action = Some(WorktreeAction::ToggleCollapse);
                }
            });
        let panel_width = panel_response.response.rect.width();
        return WorktreePanelResult { action, panel_width };
    }

    let panel_response = egui::SidePanel::left("worktree_panel")
        .resizable(false)
        .exact_width(sidebar_width)
        .frame(panel_frame)
        .show(ctx, |ui| {
            // No drag handle — sidebar uses collapse/expand toggle button
            // Header row: action buttons only
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Pull/sync button — animated pulse when remote has updates
                    let pull_color = if has_remote_updates {
                        // Pulsing green animation
                        let t = ui.ctx().input(|i| i.time);
                        let pulse = ((t * 3.0).sin() * 0.5 + 0.5) as f32; // 0..1 oscillation
                        let r = (78.0 + pulse * 60.0) as u8;
                        let g = (154.0 + pulse * 80.0) as u8;
                        let b = (6.0 + pulse * 30.0) as u8;
                        ui.ctx().request_repaint(); // keep animating
                        Color32::from_rgb(r, g, b)
                    } else {
                        Color32::from_rgb(150, 150, 150)
                    };
                    let pull_tooltip = if has_remote_updates {
                        "Remote has new changes - click to pull"
                    } else {
                        "Sync with remote (every 60s)"
                    };
                    let sync_icon = if has_remote_updates { icons::REPO_PULL } else { icons::SYNC };
                    if ui.add(egui::Button::new(
                        RichText::new(sync_icon).color(pull_color).size(11.0)
                    ).small().frame(false)).on_hover_text(pull_tooltip).clicked() {
                        if has_remote_updates {
                            action = Some(WorktreeAction::PullRemote);
                        } else {
                            action = Some(WorktreeAction::Refresh);
                        }
                    }
                    if ui.add(egui::Button::new(
                        RichText::new(icons::ADD).size(11.0)
                    ).small().frame(false)).on_hover_text("New worktree").clicked() {
                        action = Some(WorktreeAction::CreateNew);
                    }
                });
            });

            ui.separator();

            ui.horizontal(|ui| {
                ui.label(RichText::new("Worktrees").size(12.0).color(Color32::GRAY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = ui.add(
                        egui::Button::new(icons::CHEVRON_LEFT)
                            .small()
                            .sense(egui::Sense::click_and_drag()),
                    ).on_hover_text("Click to collapse, drag to resize");
                    if btn.clicked() {
                        action = Some(WorktreeAction::ToggleCollapse);
                    }
                    if btn.dragged() {
                        let delta = btn.drag_delta().x;
                        let new_width = (sidebar_width + delta).clamp(120.0, 400.0);
                        action = Some(WorktreeAction::ResizeSidebar(new_width));
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                    }
                    if btn.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                    }
                });
            });
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                for (i, wt) in worktrees.iter().enumerate() {
                    let is_selected = selected_idx == Some(i);
                    let branch_name = wt
                        .branch
                        .as_deref()
                        .unwrap_or("(detached)");

                    // First worktree is always the primary repo (main worktree)
                    let is_main = i == 0 || matches!(branch_name, "main" | "master");

                    let bg = if is_selected {
                        Color32::from_rgb(55, 55, 60)
                    } else {
                        Color32::TRANSPARENT
                    };

                    let text_color = if is_selected {
                        Color32::WHITE
                    } else if is_main {
                        Color32::from_rgb(114, 159, 207)
                    } else {
                        Color32::from_rgb(200, 200, 200)
                    };

                    ui.horizontal(|ui| {
                        let resp = ui.selectable_label(
                            is_selected,
                            RichText::new(format!("{} {}", icons::GIT_BRANCH, branch_name))
                                .color(text_color),
                        );

                        if resp.clicked() {
                            action = Some(WorktreeAction::Select(i));
                        }

                        // Delete button on the right (not for main)
                        if !is_main {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(icons::TRASH).on_hover_text("Delete worktree").clicked() {
                                    action = Some(WorktreeAction::Delete(i));
                                }
                            });
                        }
                    });
                }
            });
        });

    let panel_rect = panel_response.response.rect;
    let panel_width = panel_rect.width();


    WorktreePanelResult {
        action,
        panel_width,
    }
}
