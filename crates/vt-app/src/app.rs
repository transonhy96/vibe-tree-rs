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

struct WorktreeTerminal {
    terminal: TerminalInstance,
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

    project_path: Option<PathBuf>,
    project_name: String,
    worktrees: Vec<Worktree>,
    selected_worktree_idx: Option<usize>,
    terminals: HashMap<PathBuf, WorktreeTerminal>,

    show_new_branch_dialog: bool,
    new_branch_name: String,
    sidebar_width: f32,
    sidebar_collapsed: bool,
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
            project_path: None,
            project_name: String::new(),
            worktrees: Vec::new(),
            selected_worktree_idx: None,
            terminals: HashMap::new(),
            show_new_branch_dialog: false,
            new_branch_name: String::new(),
            sidebar_width: 200.0,
            sidebar_collapsed: false,
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
                    &gpu.device,
                    gpu.surface_format(),
                    None,
                    1,
                    false,
                );
                let terminal_renderer = TerminalRenderer::new(
                    &gpu.device,
                    &gpu.queue,
                    gpu.surface_format(),
                    self.config.terminal.font_size,
                );
                self.theme_colors
                    .apply_to_egui(&self.egui_ctx, self.config.theme);
                self.egui_state = Some(egui_state);
                self.egui_renderer = Some(egui_renderer);
                self.terminal_renderer = Some(terminal_renderer);
                self.gpu = Some(gpu);
                tracing::info!("GPU initialized");
            }
            Err(e) => tracing::error!("GPU init failed: {}", e),
        }
    }

    fn open_project(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();
        tracing::info!(path = %path.display(), "Opening project");
        let worktrees = self
            .rt
            .block_on(async { vt_git::list_worktrees(&path).await.unwrap_or_default() });
        tracing::info!(count = worktrees.len(), "Loaded worktrees");
        self.project_path = Some(path);
        self.project_name = name;
        self.worktrees = worktrees;
        if !self.worktrees.is_empty() {
            self.select_worktree(0);
        }
    }

    fn select_worktree(&mut self, idx: usize) {
        if idx >= self.worktrees.len() {
            return;
        }
        self.selected_worktree_idx = Some(idx);
        let wt_path = self.worktrees[idx].path.clone();
        if !self.terminals.contains_key(&wt_path) {
            self.spawn_terminal_for(&wt_path);
        }
    }

    fn spawn_terminal_for(&mut self, worktree_path: &PathBuf) {
        if let Some((cw, ch)) = self
            .terminal_renderer
            .as_ref()
            .map(|r| (r.cell_width, r.cell_height))
        {
            if let Some(gpu) = &self.gpu {
                self.terminal_size = self.calc_terminal_size(
                    gpu.config.width as f32,
                    gpu.config.height as f32,
                    cw,
                    ch,
                );
            }
        }
        let proxy = self.proxy.clone();
        let wakeup = Arc::new(move || {
            let _ = proxy.send_event(AppEvent::Redraw);
        });
        let terminal = TerminalInstance::new(
            self.terminal_size.0,
            self.terminal_size.1,
            worktree_path,
            wakeup,
        );
        tracing::info!(path = %worktree_path.display(), "Terminal spawned");
        self.terminals
            .insert(worktree_path.clone(), WorktreeTerminal { terminal });
    }

    fn active_wt_path(&self) -> Option<PathBuf> {
        self.selected_worktree_idx
            .and_then(|idx| self.worktrees.get(idx))
            .map(|wt| wt.path.clone())
    }

    fn refresh_worktrees(&mut self) {
        if let Some(path) = self.project_path.clone() {
            self.worktrees = self
                .rt
                .block_on(async { vt_git::list_worktrees(&path).await.unwrap_or_default() });
        }
    }

    fn create_worktree(&mut self, branch_name: &str) {
        let Some(path) = self.project_path.clone() else {
            return;
        };
        match self
            .rt
            .block_on(vt_git::add_worktree(&path, branch_name))
        {
            Ok(res) => {
                tracing::info!(branch = %res.branch, "Worktree created");
                self.refresh_worktrees();
                if let Some(idx) = self.worktrees.iter().position(|w| w.path == res.path) {
                    self.select_worktree(idx);
                }
            }
            Err(e) => tracing::error!("Create worktree failed: {}", e),
        }
    }

    fn setup_cursor_blink(&self) {
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(530));
            loop {
                interval.tick().await;
                if proxy.send_event(AppEvent::CursorBlink).is_err() {
                    break;
                }
            }
        });
    }

    fn calc_terminal_size(&self, w: f32, h: f32, cw: f32, ch: f32) -> (u16, u16) {
        let header = 60.0_f32;
        let sidebar = if self.project_path.is_some() && !self.sidebar_collapsed {
            self.sidebar_width + 10.0
        } else {
            0.0
        };
        let cols = ((w - sidebar).max(cw) / cw).floor() as u16;
        let rows = ((h - header).max(ch) / ch).floor() as u16;
        (cols.max(2), rows.max(1))
    }

    fn handle_resize(&mut self, width: u32, height: u32) {
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(width, height);
        }
        if let Some((cw, ch)) = self
            .terminal_renderer
            .as_ref()
            .map(|r| (r.cell_width, r.cell_height))
        {
            let new_size = self.calc_terminal_size(width as f32, height as f32, cw, ch);
            if new_size != self.terminal_size {
                self.terminal_size = new_size;
                for wt in self.terminals.values_mut() {
                    wt.terminal.resize(new_size.0, new_size.1);
                }
            }
        }
    }

    fn do_frame(&mut self) {
        // 1. Process active terminal events
        if let Some(path) = self.active_wt_path() {
            if let Some(wt) = self.terminals.get_mut(&path) {
                wt.terminal.process_events();
            }
        }

        // 2. Get GPU surface
        let gpu = match &self.gpu {
            Some(g) => g,
            None => return,
        };
        let output = match gpu.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = gpu.window.inner_size();
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                return;
            }
            Err(e) => {
                tracing::error!("Surface error: {}", e);
                return;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // 3. Run egui ONCE — collect actions and rendering data
        let raw_input = self
            .egui_state
            .as_mut()
            .unwrap()
            .take_egui_input(&gpu.window);

        // Snapshot state for the egui closure (avoids borrowing self)
        let has_project = self.project_path.is_some();
        let worktrees = self.worktrees.clone();
        let selected_idx = self.selected_worktree_idx;
        let project_name = self.project_name.clone();
        let term_size = self.terminal_size;
        let has_terminal = self
            .active_wt_path()
            .map(|p| self.terminals.contains_key(&p))
            .unwrap_or(false);
        let show_new_branch = self.show_new_branch_dialog;
        let mut new_branch = self.new_branch_name.clone();

        // Action flags
        let mut open_project = false;
        let mut wt_action: Option<WorktreeAction> = None;
        let mut new_sidebar_width: Option<f32> = None;
        let mut create_branch = false;
        let mut cancel_dialog = false;
        let mut sidebar_collapsed = self.sidebar_collapsed;
        let mut toggle_sidebar = false;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let panel_frame =
                egui::Frame::new().fill(egui::Color32::from_rgb(37, 37, 38));

            // Menu bar
            egui::TopBottomPanel::top("menu_bar")
                .frame(panel_frame)
                .show(ctx, |ui| {
                    egui::menu::bar(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            if ui.button("Open Project...").clicked() {
                                open_project = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Quit").clicked() {
                                std::process::exit(0);
                            }
                        });
                    });
                });

            // Header
            egui::TopBottomPanel::top("header")
                .frame(panel_frame)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        // Sidebar toggle
                        if has_project {
                            let icon = if sidebar_collapsed { ">>" } else { "<<" };
                            if ui.small_button(icon).on_hover_text("Toggle sidebar").clicked() {
                                toggle_sidebar = true;
                            }
                            ui.separator();
                        }
                        ui.heading(
                            egui::RichText::new("VibeTreeRS")
                                .color(egui::Color32::from_rgb(66, 133, 244)),
                        );
                        if has_project {
                            ui.separator();
                            ui.label(&project_name);
                        }
                        if has_terminal {
                            ui.separator();
                            ui.label(format!("{}x{}", term_size.0, term_size.1));
                        }
                    });
                });

            // Worktree sidebar (only when not collapsed)
            if has_project && !sidebar_collapsed {
                let result = draw_worktree_panel(ctx, &worktrees, selected_idx, &project_name);
                wt_action = result.action;
                new_sidebar_width = Some(result.panel_width);
            }

            // Central panel (transparent for terminal)
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    if !has_project {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.heading(
                                egui::RichText::new("VibeTreeRS")
                                    .size(32.0)
                                    .color(egui::Color32::from_rgb(66, 133, 244)),
                            );
                            ui.label("Vibe code with AI in parallel git worktrees");
                            ui.add_space(20.0);
                            if ui.button("Open Project Folder...").clicked() {
                                open_project = true;
                            }
                        });
                    }
                });

            // New branch dialog
            if show_new_branch {
                egui::Window::new("New Worktree")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Branch name:");
                        let resp = ui.text_edit_singleline(&mut new_branch);
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !new_branch.is_empty()
                        {
                            create_branch = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Create").clicked() && !new_branch.is_empty() {
                                create_branch = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel_dialog = true;
                            }
                        });
                    });
            }
        });

        self.new_branch_name = new_branch;

        // 4. Handle egui platform output
        self.egui_state
            .as_mut()
            .unwrap()
            .handle_platform_output(&gpu.window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        // Update sidebar width from egui layout
        if let Some(w) = new_sidebar_width {
            self.sidebar_width = w;
        }

        // 5. Prepare terminal text
        let active_term = self
            .active_wt_path()
            .and_then(|p| self.terminals.get(&p))
            .map(|wt| wt.terminal.term.clone());

        if let Some(term) = &active_term {
            if let Some(renderer) = &mut self.terminal_renderer {
                let term_offset_x = if self.project_path.is_some() {
                    self.sidebar_width + 10.0
                } else {
                    0.0
                };
                renderer.prepare(
                    term,
                    &gpu.device,
                    &gpu.queue,
                    gpu.config.width,
                    gpu.config.height,
                    term_offset_x,
                    60.0,
                );
            }
        }

        // 6. GPU render pass
        let egui_renderer = self.egui_renderer.as_mut().unwrap();
        for (id, delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });

        let _cmds = egui_renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        let bg = &self.theme_colors.terminal_bg;
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0],
                            g: bg[1],
                            b: bg[2],
                            a: bg[3],
                        }),
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

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        // 7. Process deferred actions AFTER rendering (no GPU borrows)
        if open_project {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Select a Git Project Folder")
                .pick_folder()
            {
                self.open_project(path);
            }
        }
        if let Some(action) = wt_action {
            match action {
                WorktreeAction::Select(idx) => self.select_worktree(idx),
                WorktreeAction::Refresh => self.refresh_worktrees(),
                WorktreeAction::CreateNew => {
                    self.show_new_branch_dialog = true;
                    self.new_branch_name.clear();
                }
                WorktreeAction::Delete(_idx) => {
                    tracing::info!("Delete worktree requested");
                }
            }
        }
        if create_branch {
            let name = self.new_branch_name.clone();
            self.create_worktree(&name);
            self.show_new_branch_dialog = false;
            self.new_branch_name.clear();
        }
        if cancel_dialog {
            self.show_new_branch_dialog = false;
        }
        if toggle_sidebar {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            if self.sidebar_collapsed {
                self.sidebar_width = 0.0;
            }
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("VibeTreeRS")
            .with_inner_size(winit::dpi::LogicalSize::new(1200, 800));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                self.initialize_gpu(window);
                self.setup_cursor_blink();
                let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
                self.open_project(cwd);
            }
            Err(e) => tracing::error!("Window creation failed: {}", e),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_keyboard = matches!(event, WindowEvent::KeyboardInput { .. });
        if let Some(egui_state) = &mut self.egui_state {
            if let Some(gpu) = &self.gpu {
                let response = egui_state.on_window_event(&gpu.window, &event);
                // Only let egui consume keyboard when a dialog/text field is active
                let egui_needs_kb = self.show_new_branch_dialog;
                if response.consumed && (!is_keyboard || egui_needs_kb) {
                    if response.repaint {
                        gpu.window.request_redraw();
                    }
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.terminals.clear();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.handle_resize(size.width, size.height);
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.do_frame();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        ref logical_key,
                        ref text,
                        ..
                    },
                ..
            } => {
                // Mark cursor as active on first keypress
                if let Some(renderer) = &mut self.terminal_renderer {
                    renderer.mark_input();
                }
                if let Some(path) = self.active_wt_path() {
                    if let Some(wt) = self.terminals.get(&path) {
                        match logical_key {
                            Key::Named(NamedKey::Enter) => wt.terminal.write(b"\r"),
                            Key::Named(NamedKey::Backspace) => wt.terminal.write(b"\x7f"),
                            Key::Named(NamedKey::Tab) => wt.terminal.write(b"\t"),
                            Key::Named(NamedKey::Escape) => wt.terminal.write(b"\x1b"),
                            Key::Named(NamedKey::ArrowUp) => wt.terminal.write(b"\x1b[A"),
                            Key::Named(NamedKey::ArrowDown) => wt.terminal.write(b"\x1b[B"),
                            Key::Named(NamedKey::ArrowRight) => wt.terminal.write(b"\x1b[C"),
                            Key::Named(NamedKey::ArrowLeft) => wt.terminal.write(b"\x1b[D"),
                            Key::Named(NamedKey::Home) => wt.terminal.write(b"\x1b[H"),
                            Key::Named(NamedKey::End) => wt.terminal.write(b"\x1b[F"),
                            Key::Named(NamedKey::PageUp) => wt.terminal.write(b"\x1b[5~"),
                            Key::Named(NamedKey::PageDown) => wt.terminal.write(b"\x1b[6~"),
                            Key::Named(NamedKey::Delete) => wt.terminal.write(b"\x1b[3~"),
                            _ => {
                                if let Some(text) = text {
                                    wt.terminal.write(text.as_bytes());
                                }
                            }
                        }
                    }
                }
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                // Clicking the terminal area activates cursor
                if let Some(renderer) = &mut self.terminal_renderer {
                    renderer.mark_input();
                }
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        // 3 terminal lines per scroll notch
                        (y * 3.0) as i32
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        let ch = self.terminal_renderer.as_ref()
                            .map(|r| r.cell_height as f64)
                            .unwrap_or(19.0);
                        (pos.y / ch) as i32
                    }
                };
                if lines != 0 {
                    if let Some(path) = self.active_wt_path() {
                        if let Some(wt) = self.terminals.get(&path) {
                            wt.terminal.scroll(lines);
                        }
                    }
                    if let Some(gpu) = &self.gpu {
                        gpu.window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redraw | AppEvent::PtyOutput { .. } => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            AppEvent::CursorBlink => {
                // Only redraw if cursor blink is active (user has typed)
                if let Some(renderer) = &mut self.terminal_renderer {
                    if renderer.toggle_cursor_blink() {
                        if let Some(gpu) = &self.gpu {
                            gpu.window.request_redraw();
                        }
                    }
                }
            }
            AppEvent::PtyExited { session_id, code } => {
                tracing::info!(session_id, code, "PTY exited");
            }
        }
    }
}
