use egui::{self, Color32, RichText};
use vt_core::types::Worktree;

/// Actions emitted by the worktree panel.
pub enum WorktreeAction {
    Select(usize),
    CreateNew,
    Delete(usize),
    Refresh,
    PullRemote,
    ToggleCollapse,
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
                if ui.button(">").on_hover_text("Expand sidebar").clicked() {
                    action = Some(WorktreeAction::ToggleCollapse);
                }
            });
        let panel_width = panel_response.response.rect.width();
        return WorktreePanelResult { action, panel_width };
    }

    let panel_response = egui::SidePanel::left("worktree_panel")
        .resizable(false)
        .exact_width(200.0)
        .frame(panel_frame)
        .show(ctx, |ui| {
            // Header row: action buttons only
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Pull/sync button — glows green when remote has updates
                    let pull_color = if has_remote_updates {
                        Color32::from_rgb(78, 154, 6) // green glow
                    } else {
                        Color32::from_rgb(150, 150, 150)
                    };
                    let pull_text = if has_remote_updates { "Pull" } else { "Sync" };
                    let pull_tooltip = if has_remote_updates {
                        "Remote has new changes - click to pull"
                    } else {
                        "Check and pull remote changes"
                    };
                    if ui.add(egui::Button::new(
                        RichText::new(pull_text).color(pull_color).size(11.0)
                    ).small()).on_hover_text(pull_tooltip).clicked() {
                        if has_remote_updates {
                            action = Some(WorktreeAction::PullRemote);
                        } else {
                            action = Some(WorktreeAction::Refresh);
                        }
                    }
                    if ui.small_button("+").on_hover_text("New worktree").clicked() {
                        action = Some(WorktreeAction::CreateNew);
                    }
                    // Collapse button (leftmost in right-to-left layout)
                    let collapse_icon = if collapsed { ">" } else { "<" };
                    if ui.small_button(collapse_icon).on_hover_text("Toggle sidebar").clicked() {
                        action = Some(WorktreeAction::ToggleCollapse);
                    }
                });
            });

            ui.separator();

            ui.label(RichText::new("Worktrees").size(12.0).color(Color32::GRAY));
            ui.add_space(4.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
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
                            RichText::new(format!("{} {}", if is_main { "*" } else { "-" }, branch_name))
                                .color(text_color),
                        );

                        if resp.clicked() {
                            action = Some(WorktreeAction::Select(i));
                        }

                        // Delete button on the right (not for main)
                        if !is_main {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("x").on_hover_text("Delete worktree").clicked() {
                                    action = Some(WorktreeAction::Delete(i));
                                }
                            });
                        }
                    });
                }
            });
        });

    let panel_width = panel_response.response.rect.width();

    WorktreePanelResult {
        action,
        panel_width,
    }
}
