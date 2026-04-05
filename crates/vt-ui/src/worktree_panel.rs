use egui::{self, Color32, RichText, Vec2};
use vt_core::types::Worktree;

/// Actions emitted by the worktree panel.
pub enum WorktreeAction {
    Select(usize),
    CreateNew,
    Delete(usize),
    Refresh,
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
) -> WorktreePanelResult {
    let mut action = None;

    let panel_frame = egui::Frame::new()
        .fill(Color32::from_rgb(37, 37, 38))
        .inner_margin(egui::Margin::same(8));

    let panel_response = egui::SidePanel::left("worktree_panel")
        .resizable(true)
        .default_width(200.0)
        .min_width(150.0)
        .max_width(400.0)
        .frame(panel_frame)
        .show(ctx, |ui| {
            // Header row: project name + add + refresh buttons
            ui.horizontal(|ui| {
                ui.strong(RichText::new(project_name).color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("R").on_hover_text("Refresh worktrees").clicked() {
                        action = Some(WorktreeAction::Refresh);
                    }
                    if ui.small_button("+").on_hover_text("New worktree").clicked() {
                        action = Some(WorktreeAction::CreateNew);
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

                    let is_main = matches!(branch_name, "main" | "master");

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

                    let response = ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let rect = ui.max_rect();
                            ui.painter().rect_filled(rect, 4.0, bg);

                            ui.add_space(8.0);

                            let icon = if is_main { "*" } else { "-" };
                            ui.label(RichText::new(icon).color(text_color).monospace());

                            let resp = ui.selectable_label(
                                false,
                                RichText::new(branch_name).color(text_color),
                            );

                            if resp.clicked() {
                                return Some(i);
                            }
                            None
                        },
                    );

                    if let Some(idx) = response.inner {
                        action = Some(WorktreeAction::Select(idx));
                    }

                    // Right-click context menu for non-main branches
                    if !is_main {
                        response.response.context_menu(|ui| {
                            if ui.button("Delete worktree").clicked() {
                                action = Some(WorktreeAction::Delete(i));
                                ui.close_menu();
                            }
                        });
                    }
                }
            });
        });

    let panel_width = panel_response.response.rect.width();

    WorktreePanelResult {
        action,
        panel_width,
    }
}
