use crate::event::AppEvent;
use crate::gpu::GpuContext;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use vt_core::config::AppConfig;
use vt_core::types::Worktree;
use vt_terminal::{TerminalInstance, TerminalRenderer};
use vt_ui::{draw_worktree_panel, ThemeColors, WorktreeAction, WorktreePanelResult};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

struct Workspace {
    path: PathBuf,
    name: String,
    worktrees: Vec<Worktree>,
    selected_worktree_idx: Option<usize>,
    terminals: HashMap<PathBuf, TerminalInstance>,
    has_remote_updates: bool,
    default_branch: Option<String>,
    sidebar_collapsed: bool,
}

pub struct App {
    rt: tokio::runtime::Runtime,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    gpu: Option<GpuContext>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    terminal_renderer: Option<TerminalRenderer>,
    config: AppConfig,
    theme_colors: ThemeColors,
    terminal_size: (u16, u16),

    workspaces: Vec<Workspace>,
    active_workspace_idx: Option<usize>,

    show_new_branch_dialog: bool,
    new_branch_name: String,
    show_open_project_dialog: bool,
    open_project_path: String,
    open_project_error: Option<String>,
    sidebar_width: f32,
}

impl App {
    pub fn new(
        rt: tokio::runtime::Runtime,
        proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    ) -> Self {
        let config = AppConfig::load();
        let theme_colors = ThemeColors::from_theme(config.theme);
        Self {
            rt,
            proxy,
            gpu: None,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            egui_renderer: None,
            terminal_renderer: None,
            config,
            theme_colors,
            terminal_size: (80, 24),
            workspaces: Vec::new(),
            active_workspace_idx: None,
            show_new_branch_dialog: false,
            new_branch_name: String::new(),
            show_open_project_dialog: false,
            open_project_path: String::new(),
            open_project_error: None,
            sidebar_width: 200.0,
        }
    }

    fn initialize_gpu(&mut self, window: Arc<Window>) {
        let gpu = self.rt.block_on(GpuContext::new(window.clone()));
        match gpu {
            Ok(gpu) => {
                let egui_state = egui_winit::State::new(
                    self.egui_ctx.clone(),
                    self.egui_ctx.viewport_id(),
                    &gpu.window,
                    None,
                    None,
                    None,
                );
                let egui_renderer = egui_wgpu::Renderer::new(
                    &gpu.device, gpu.surface_format(), None, 1, false,
                );
                let terminal_renderer = TerminalRenderer::new(
                    &gpu.device, &gpu.queue, gpu.surface_format(),
                    self.config.terminal.font_size,
                );
                self.theme_colors.apply_to_egui(&self.egui_ctx, self.config.theme);
                self.egui_state = Some(egui_state);
                self.egui_renderer = Some(egui_renderer);
                self.terminal_renderer = Some(terminal_renderer);
                self.gpu = Some(gpu);
            }
            Err(e) => tracing::error!("GPU init failed: {}", e),
        }
    }

    fn open_workspace(&mut self, path: PathBuf) {
        // Don't open duplicate
        if self.workspaces.iter().any(|w| w.path == path) {
            self.active_workspace_idx = self.workspaces.iter().position(|w| w.path == path);
            return;
        }

        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();

        let mut worktrees = self.rt.block_on(async {
            vt_git::list_worktrees(&path).await.unwrap_or_default()
        });

        // If main/master exists as a branch but not as a worktree, add it.
        // Clicking it will create a real worktree.
        let has_main = worktrees.iter().any(|w| {
            w.branch.as_deref().map(|b| matches!(b, "main" | "master")).unwrap_or(false)
        });
        if !has_main {
            if let Some(branch) = self.rt.block_on(vt_git::get_default_branch(&path)) {
                let wt_dir = format!("{}-{}", name, branch);
                let wt_path = path.parent().unwrap_or(&path).join(&wt_dir);
                worktrees.push(Worktree {
                    path: wt_path,
                    branch: Some(branch),
                    head: String::new(),
                });
            }
        }

        // Sort: main/master first, then alphabetical
        worktrees.sort_by(|a, b| {
            let a_main = a.branch.as_deref().map(|b| matches!(b, "main" | "master")).unwrap_or(false);
            let b_main = b.branch.as_deref().map(|b| matches!(b, "main" | "master")).unwrap_or(false);
            b_main.cmp(&a_main).then_with(|| a.branch.cmp(&b.branch))
        });

        let default_branch = self.rt.block_on(vt_git::get_default_branch(&path))
            .unwrap_or_else(|| "main".to_string());

        tracing::info!(path = %path.display(), name = %name, worktrees = worktrees.len(), "Workspace opened");

        let ws = Workspace {
            path: path.clone(),
            name,
            worktrees,
            selected_worktree_idx: None,
            terminals: HashMap::new(),
            has_remote_updates: false,
            default_branch: Some(default_branch.clone()),
            sidebar_collapsed: false,
        };
        self.workspaces.push(ws);
        let idx = self.workspaces.len() - 1;
        self.active_workspace_idx = Some(idx);

        // Start background remote check every 5 minutes
        self.start_remote_check(idx, path, default_branch);

        // Auto-select first worktree (main if sorted correctly)
        if !self.workspaces[idx].worktrees.is_empty() {
            self.select_worktree(0);
        }
        self.save_state();
    }

    fn close_workspace(&mut self, idx: usize) {
        if idx >= self.workspaces.len() { return; }
        self.workspaces[idx].terminals.clear();
        self.workspaces.remove(idx);
        if self.workspaces.is_empty() {
            self.active_workspace_idx = None;
        } else {
            self.active_workspace_idx = Some(idx.min(self.workspaces.len() - 1));
        }
        self.save_state();
    }

    fn save_state(&mut self) {
        self.config.workspace_state.open_paths = self.workspaces.iter()
            .map(|ws| ws.path.clone())
            .collect();
        self.config.workspace_state.active_index = self.active_workspace_idx;
        if let Err(e) = self.config.save() {
            tracing::error!("Failed to save config: {}", e);
        }
    }

    fn restore_workspaces(&mut self) {
        let paths = self.config.workspace_state.open_paths.clone();
        let active = self.config.workspace_state.active_index;
        for path in paths {
            if path.is_dir() {
                self.open_workspace(path);
            }
        }
        if let Some(idx) = active {
            if idx < self.workspaces.len() {
                self.active_workspace_idx = Some(idx);
                // Select the first worktree in the restored active workspace
                if let Some(ws) = self.workspaces.get(idx) {
                    if !ws.worktrees.is_empty() && ws.selected_worktree_idx.is_none() {
                        self.select_worktree(0);
                    }
                }
            }
        }
    }

    fn active_ws(&self) -> Option<&Workspace> {
        self.active_workspace_idx.and_then(|i| self.workspaces.get(i))
    }

    fn active_ws_mut(&mut self) -> Option<&mut Workspace> {
        self.active_workspace_idx.and_then(|i| self.workspaces.get_mut(i))
    }

    fn select_worktree(&mut self, idx: usize) {
        let ws = match self.active_ws_mut() {
            Some(ws) => ws,
            None => return,
        };
        if idx >= ws.worktrees.len() { return; }

        let wt_path = ws.worktrees[idx].path.clone();
        let wt_branch = ws.worktrees[idx].branch.clone();

        // If worktree dir doesn't exist, create it (virtual entry like main)
        if !wt_path.exists() {
            if let Some(branch_name) = &wt_branch {
                let project_path = ws.path.clone();
                tracing::info!(branch = %branch_name, "Creating worktree");
                match self.rt.block_on(vt_git::add_worktree(&project_path, branch_name)) {
                    Ok(res) => {
                        tracing::info!(path = %res.path.display(), "Worktree created");
                        self.refresh_worktrees();
                        // Find and select the newly created worktree
                        if let Some(ws) = self.active_ws() {
                            if let Some(new_idx) = ws.worktrees.iter().position(|w| w.path == res.path) {
                                self.select_worktree(new_idx); // recurse with real path
                                return;
                            }
                        }
                    }
                    Err(e) => tracing::error!("Failed to create worktree: {}", e),
                }
            }
            return;
        }

        let ws = self.active_ws_mut().unwrap();
        ws.selected_worktree_idx = Some(idx);

        if !ws.terminals.contains_key(&wt_path) {
            let proxy = self.proxy.clone();
            let wakeup = Arc::new(move || { let _ = proxy.send_event(AppEvent::Redraw); });

            let cell_dims = self.terminal_renderer.as_ref()
                .map(|r| (r.cell_width, r.cell_height));
            if let Some((cw, ch)) = cell_dims {
                if let Some(gpu) = &self.gpu {
                    self.terminal_size = self.calc_terminal_size(
                        gpu.config.width as f32, gpu.config.height as f32, cw, ch,
                    );
                }
            }

            let terminal = TerminalInstance::new(
                self.terminal_size.0, self.terminal_size.1, &wt_path, wakeup,
            );
            tracing::info!(path = %wt_path.display(), "Terminal spawned");

            // Re-borrow after creating terminal (can't hold ws across TerminalInstance::new)
            let ws = self.active_ws_mut().unwrap();
            ws.terminals.insert(wt_path, terminal);
        }
    }

    fn start_remote_check(&self, ws_idx: usize, path: PathBuf, branch: String) {
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            loop {
                // Wait 5 minutes
                tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
                // Fetch and check
                let _ = vt_git::fetch(&path).await;
                if vt_git::has_remote_changes(&path, &branch).await {
                    tracing::info!("Remote updates available for {}", branch);
                    if proxy.send_event(AppEvent::RemoteUpdatesAvailable { workspace_idx: ws_idx }).is_err() {
                        break;
                    }
                }
            }
        });
    }

    fn pull_remote(&mut self) {
        let ws = match self.active_ws_mut() {
            Some(ws) => ws,
            None => return,
        };
        let path = ws.path.clone();
        match self.rt.block_on(vt_git::pull(&path)) {
            Ok(output) => {
                tracing::info!("Pull: {}", output.trim());
                let ws = self.active_ws_mut().unwrap();
                ws.has_remote_updates = false;
                self.refresh_worktrees();
            }
            Err(e) => tracing::error!("Pull failed: {}", e),
        }
    }

    fn refresh_worktrees(&mut self) {
        let ws = match self.active_ws_mut() {
            Some(ws) => ws,
            None => return,
        };
        let path = ws.path.clone();
        let mut worktrees = self.rt.block_on(async {
            vt_git::list_worktrees(&path).await.unwrap_or_default()
        });
        worktrees.sort_by(|a, b| {
            let a_main = a.branch.as_deref().map(|b| matches!(b, "main" | "master")).unwrap_or(false);
            let b_main = b.branch.as_deref().map(|b| matches!(b, "main" | "master")).unwrap_or(false);
            b_main.cmp(&a_main).then_with(|| a.branch.cmp(&b.branch))
        });
        let ws = self.active_ws_mut().unwrap();
        ws.worktrees = worktrees;
    }

    fn create_worktree(&mut self, branch_name: &str) {
        let project_path = match self.active_ws() {
            Some(ws) => ws.path.clone(),
            None => return,
        };
        match self.rt.block_on(vt_git::add_worktree(&project_path, branch_name)) {
            Ok(res) => {
                tracing::info!(branch = %res.branch, "Worktree created");
                self.refresh_worktrees();
                let new_path = res.path;
                if let Some(ws) = self.active_ws() {
                    if let Some(idx) = ws.worktrees.iter().position(|w| w.path == new_path) {
                        self.select_worktree(idx);
                    }
                }
            }
            Err(e) => tracing::error!("Create worktree failed: {}", e),
        }
    }

    fn active_terminal(&self) -> Option<&TerminalInstance> {
        let ws = self.active_ws()?;
        let idx = ws.selected_worktree_idx?;
        let wt_path = &ws.worktrees.get(idx)?.path;
        ws.terminals.get(wt_path)
    }

    fn calc_terminal_size(&self, w: f32, h: f32, cw: f32, ch: f32) -> (u16, u16) {
        let header = 80.0_f32; // tabs + header
        let sidebar = if self.active_ws().is_some() { self.sidebar_width } else { 0.0 };
        let cols = ((w - sidebar).max(cw) / cw).floor() as u16;
        let rows = ((h - header).max(ch) / ch).floor() as u16;
        (cols.max(2), rows.max(1))
    }

    fn handle_resize(&mut self, width: u32, height: u32) {
        if let Some(gpu) = &mut self.gpu { gpu.resize(width, height); }
        if let Some((cw, ch)) = self.terminal_renderer.as_ref().map(|r| (r.cell_width, r.cell_height)) {
            let new_size = self.calc_terminal_size(width as f32, height as f32, cw, ch);
            if new_size != self.terminal_size {
                self.terminal_size = new_size;
                for ws in &mut self.workspaces {
                    for t in ws.terminals.values_mut() {
                        t.resize(new_size.0, new_size.1);
                    }
                }
            }
        }
    }

    fn do_frame(&mut self) {
        // Process active terminal events
        if let Some(ws) = self.active_ws_mut() {
            if let Some(idx) = ws.selected_worktree_idx {
                if let Some(wt) = ws.worktrees.get(idx) {
                    let path = wt.path.clone();
                    if let Some(term) = ws.terminals.get_mut(&path) {
                        term.process_events();
                    }
                }
            }
        }

        let gpu = match &self.gpu { Some(g) => g, None => return };
        let output = match gpu.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = gpu.window.inner_size();
                if let Some(gpu) = &mut self.gpu { gpu.resize(size.width, size.height); }
                return;
            }
            Err(e) => { tracing::error!("Surface error: {}", e); return; }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Take egui input
        let raw_input = self.egui_state.as_mut().unwrap().take_egui_input(&gpu.window);

        // Snapshot state
        let ws_names: Vec<String> = self.workspaces.iter().map(|w| w.name.clone()).collect();
        let active_ws_idx = self.active_workspace_idx;
        let has_workspace = active_ws_idx.is_some();
        let worktrees = self.active_ws().map(|ws| ws.worktrees.clone()).unwrap_or_default();
        let selected_wt_idx = self.active_ws().and_then(|ws| ws.selected_worktree_idx);
        let project_name = self.active_ws().map(|ws| ws.name.clone()).unwrap_or_default();
        let term_size = self.terminal_size;
        let has_terminal = self.active_terminal().is_some();
        let has_remote_updates = self.active_ws().map(|ws| ws.has_remote_updates).unwrap_or(false);
        let sidebar_collapsed = self.active_ws().map(|ws| ws.sidebar_collapsed).unwrap_or(false);
        let show_new_branch = self.show_new_branch_dialog;
        let mut new_branch = self.new_branch_name.clone();
        let show_open_project = self.show_open_project_dialog;
        let mut project_path_input = self.open_project_path.clone();
        let open_project_err = self.open_project_error.clone();

        // Actions
        let mut open_project = false;
        let mut confirm_open_project = false;
        let mut cancel_open_project = false;
        let mut wt_result: Option<WorktreePanelResult> = None;
        let mut create_branch = false;
        let mut cancel_dialog = false;
        let mut switch_ws: Option<usize> = None;
        let mut close_ws: Option<usize> = None;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let panel_frame = egui::Frame::new().fill(egui::Color32::from_rgb(30, 30, 30));
            let tab_frame = egui::Frame::new().fill(egui::Color32::from_rgb(24, 24, 24));

            // Header bar (top-most)
            egui::TopBottomPanel::top("header")
                .frame(panel_frame)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(
                            egui::RichText::new("VibeTreeRS")
                                .color(egui::Color32::from_rgb(66, 133, 244)),
                        );
                        if has_workspace {
                            ui.separator();
                            ui.label(&project_name);
                        }
                        if has_terminal {
                            ui.separator();
                            ui.label(format!("{}x{}", term_size.0, term_size.1));
                        }
                    });
                });

            // Workspace tab bar (below header)
            egui::TopBottomPanel::top("workspace_tabs")
                .frame(tab_frame)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        for (i, name) in ws_names.iter().enumerate() {
                            let is_active = active_ws_idx == Some(i);
                            let text_color = if is_active {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_rgb(150, 150, 150)
                            };

                            ui.horizontal(|ui| {
                                let resp = ui.selectable_label(
                                    is_active,
                                    egui::RichText::new(name).color(text_color),
                                );
                                if resp.clicked() {
                                    switch_ws = Some(i);
                                }
                                if ui.small_button("x").clicked() {
                                    close_ws = Some(i);
                                }
                            });
                            ui.separator();
                        }

                        if ui.small_button("+").on_hover_text("Open project").clicked() {
                            open_project = true;
                        }
                    });
                });

            // Worktree sidebar (only when workspace is open)
            if has_workspace {
                wt_result = Some(draw_worktree_panel(ctx, &worktrees, selected_wt_idx, &project_name, has_remote_updates, sidebar_collapsed));
            }

            // Central panel
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if !has_workspace {
                        // Welcome screen
                        ui.vertical_centered(|ui| {
                            ui.add_space(150.0);
                            ui.heading(
                                egui::RichText::new("VibeTreeRS")
                                    .size(40.0)
                                    .color(egui::Color32::from_rgb(66, 133, 244)),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Vibe code with AI in parallel git worktrees")
                                    .size(16.0)
                                    .color(egui::Color32::from_rgb(150, 150, 150)),
                            );
                            ui.add_space(30.0);
                            if ui.button(
                                egui::RichText::new("  Open Project Folder  ").size(16.0)
                            ).clicked() {
                                open_project = true;
                            }
                        });
                    }
                });

            // Open project dialog
            if show_open_project {
                egui::Window::new("Open Project")
                    .collapsible(false)
                    .resizable(false)
                    .min_width(400.0)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Enter the path to a git repository:");
                        ui.add_space(4.0);
                        let resp = ui.text_edit_singleline(&mut project_path_input);
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !project_path_input.is_empty()
                        {
                            confirm_open_project = true;
                        }
                        if let Some(err) = &open_project_err {
                            ui.colored_label(egui::Color32::RED, err);
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("Open").clicked() && !project_path_input.is_empty() {
                                confirm_open_project = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_open_project = true;
                            }
                        });
                    });
            }

            // New branch dialog
            if show_new_branch {
                let mut is_open = true;
                egui::Window::new("New Worktree")
                    .open(&mut is_open)
                    .collapsible(false)
                    .resizable(false)
                    .min_width(400.0)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Create a new git worktree branch")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(180, 180, 180)),
                        );
                        ui.add_space(12.0);
                        ui.label("Branch name:");
                        ui.add_space(4.0);
                        let resp = ui.add_sized(
                            [ui.available_width(), 28.0],
                            egui::TextEdit::singleline(&mut new_branch)
                                .hint_text("e.g. feature/my-feature"),
                        );
                        // Auto-focus the input
                        if resp.gained_focus() || new_branch.is_empty() {
                            resp.request_focus();
                        }
                        // Enter to create
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !new_branch.is_empty()
                        {
                            create_branch = true;
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui.button(
                                egui::RichText::new("  Create  ").size(14.0)
                            ).clicked() && !new_branch.is_empty() {
                                create_branch = true;
                            }
                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() {
                                cancel_dialog = true;
                            }
                        });
                        ui.add_space(4.0);
                    });
                if !is_open {
                    cancel_dialog = true;
                }
            }
        });

        self.new_branch_name = new_branch;
        self.open_project_path = project_path_input;

        // Handle egui output
        self.egui_state.as_mut().unwrap()
            .handle_platform_output(&gpu.window, full_output.platform_output);
        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        // Update sidebar width from panel result and resize terminal if needed
        if let Some(ref result) = wt_result {
            let old_width = self.sidebar_width;
            self.sidebar_width = result.panel_width;
            if (old_width - result.panel_width).abs() > 1.0 {
                // Sidebar resized — recalculate terminal cols
                if let Some((cw, ch)) = self.terminal_renderer.as_ref()
                    .map(|r| (r.cell_width, r.cell_height))
                {
                    let new_size = self.calc_terminal_size(
                        gpu.config.width as f32, gpu.config.height as f32, cw, ch,
                    );
                    if new_size != self.terminal_size {
                        self.terminal_size = new_size;
                        for ws in &mut self.workspaces {
                            for t in ws.terminals.values_mut() {
                                t.resize(new_size.0, new_size.1);
                            }
                        }
                    }
                }
            }
        }

        // Prepare terminal text
        let active_term = self.active_terminal().map(|t| t.term.clone());
        if let Some(term) = &active_term {
            if let Some(renderer) = &mut self.terminal_renderer {
                let term_offset_x = if has_workspace { self.sidebar_width } else { 0.0 };
                renderer.prepare(term, &gpu.device, &gpu.queue,
                    gpu.config.width, gpu.config.height, term_offset_x, 80.0);
            }
        }

        // GPU render
        let egui_renderer = self.egui_renderer.as_mut().unwrap();
        for (id, delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
        let mut encoder = gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor { label: Some("render") },
        );
        let _cmds = egui_renderer.update_buffers(
            &gpu.device, &gpu.queue, &mut encoder, &paint_jobs, &screen_descriptor,
        );

        let bg = &self.theme_colors.terminal_bg;
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: bg[0], g: bg[1], b: bg[2], a: bg[3] }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
            if let Some(renderer) = &self.terminal_renderer {
                renderer.render_pass(&mut render_pass);
            }
        }

        for id in &full_output.textures_delta.free { egui_renderer.free_texture(id); }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        // Process deferred actions
        if open_project {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select a Git Project Folder")
                .pick_folder()
            {
                self.open_workspace(path);
            }
        }
        if let Some(idx) = switch_ws {
            self.active_workspace_idx = Some(idx);
            self.save_state();
        }
        if let Some(idx) = close_ws { self.close_workspace(idx); }
        if let Some(action) = wt_result.and_then(|r| r.action) {
            match action {
                WorktreeAction::Select(idx) => self.select_worktree(idx),
                WorktreeAction::Refresh => self.refresh_worktrees(),
                WorktreeAction::CreateNew => {
                    self.show_new_branch_dialog = true;
                    self.new_branch_name.clear();
                }
                WorktreeAction::Delete(_) => tracing::info!("Delete worktree requested"),
                WorktreeAction::PullRemote => self.pull_remote(),
                WorktreeAction::ToggleCollapse => {
                    if let Some(ws) = self.active_ws_mut() {
                        ws.sidebar_collapsed = !ws.sidebar_collapsed;
                    }
                }
            }
        }
        if create_branch {
            let name = self.new_branch_name.clone();
            self.create_worktree(&name);
            self.show_new_branch_dialog = false;
            self.new_branch_name.clear();
        }
        if cancel_dialog { self.show_new_branch_dialog = false; }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() { return; }
        let attrs = WindowAttributes::default()
            .with_title("VibeTreeRS")
            .with_inner_size(winit::dpi::LogicalSize::new(1200, 800));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                self.initialize_gpu(window);
                // Restore previously open workspaces
                self.restore_workspaces();
            }
            Err(e) => tracing::error!("Window creation failed: {}", e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let is_keyboard = matches!(event, WindowEvent::KeyboardInput { .. });
        if let Some(egui_state) = &mut self.egui_state {
            if let Some(gpu) = &self.gpu {
                let response = egui_state.on_window_event(&gpu.window, &event);
                let egui_needs_kb = self.show_new_branch_dialog || self.show_open_project_dialog;
                // Always redraw after any event so egui UI stays responsive
                gpu.window.request_redraw();
                if response.consumed && (!is_keyboard || egui_needs_kb) {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_state();
                self.workspaces.clear();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.handle_resize(size.width, size.height);
                if let Some(gpu) = &self.gpu { gpu.window.request_redraw(); }
            }
            WindowEvent::RedrawRequested => { self.do_frame(); }
            WindowEvent::KeyboardInput {
                event: KeyEvent { state: ElementState::Pressed, ref logical_key, ref text, .. }, ..
            } => {
                if let Some(terminal) = self.active_terminal() {
                    match logical_key {
                        Key::Named(NamedKey::Enter) => terminal.write(b"\r"),
                        Key::Named(NamedKey::Backspace) => terminal.write(b"\x7f"),
                        Key::Named(NamedKey::Tab) => terminal.write(b"\t"),
                        Key::Named(NamedKey::Escape) => terminal.write(b"\x1b"),
                        Key::Named(NamedKey::ArrowUp) => terminal.write(b"\x1b[A"),
                        Key::Named(NamedKey::ArrowDown) => terminal.write(b"\x1b[B"),
                        Key::Named(NamedKey::ArrowRight) => terminal.write(b"\x1b[C"),
                        Key::Named(NamedKey::ArrowLeft) => terminal.write(b"\x1b[D"),
                        Key::Named(NamedKey::Home) => terminal.write(b"\x1b[H"),
                        Key::Named(NamedKey::End) => terminal.write(b"\x1b[F"),
                        Key::Named(NamedKey::PageUp) => terminal.write(b"\x1b[5~"),
                        Key::Named(NamedKey::PageDown) => terminal.write(b"\x1b[6~"),
                        Key::Named(NamedKey::Delete) => terminal.write(b"\x1b[3~"),
                        _ => { if let Some(text) = text { terminal.write(text.as_bytes()); } }
                    }
                }
                if let Some(gpu) = &self.gpu { gpu.window.request_redraw(); }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redraw | AppEvent::PtyOutput { .. } | AppEvent::CursorBlink => {
                if let Some(gpu) = &self.gpu { gpu.window.request_redraw(); }
            }
            AppEvent::PtyExited { session_id, code } => {
                tracing::info!(session_id, code, "PTY exited");
            }
            AppEvent::RemoteUpdatesAvailable { workspace_idx } => {
                if let Some(ws) = self.workspaces.get_mut(workspace_idx) {
                    ws.has_remote_updates = true;
                    tracing::info!(workspace = %ws.name, "Remote updates available");
                }
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        }
    }
}
