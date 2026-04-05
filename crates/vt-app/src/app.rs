use crate::event::AppEvent;
use crate::gpu::GpuContext;
use std::sync::Arc;
use vt_core::config::AppConfig;
use vt_terminal::{TerminalInstance, TerminalRenderer};
use vt_ui::ThemeColors;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

pub struct App {
    rt: tokio::runtime::Runtime,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    gpu: Option<GpuContext>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    terminal: Option<TerminalInstance>,
    terminal_renderer: Option<TerminalRenderer>,
    config: AppConfig,
    theme_colors: ThemeColors,
    /// Tracks terminal cols/rows for resize detection.
    terminal_size: (u16, u16),
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
            terminal: None,
            terminal_renderer: None,
            config,
            theme_colors,
            terminal_size: (80, 24),
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

                self.theme_colors.apply_to_egui(&self.egui_ctx, self.config.theme);

                self.egui_state = Some(egui_state);
                self.egui_renderer = Some(egui_renderer);
                self.terminal_renderer = Some(terminal_renderer);
                self.gpu = Some(gpu);

                tracing::info!("GPU initialized successfully");
            }
            Err(e) => {
                tracing::error!("Failed to initialize GPU: {}", e);
            }
        }
    }

    fn spawn_terminal(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());

        // Calculate initial terminal size from window
        if let Some(renderer) = &self.terminal_renderer {
            if let Some(gpu) = &self.gpu {
                let (cols, rows) = self.calc_terminal_size(
                    gpu.config.width as f32,
                    gpu.config.height as f32,
                    renderer.cell_width,
                    renderer.cell_height,
                );
                self.terminal_size = (cols, rows);
            }
        }

        // Create a wakeup function that sends a redraw event to winit
        let proxy = self.proxy.clone();
        let wakeup = Arc::new(move || {
            let _ = proxy.send_event(AppEvent::Redraw);
        });

        let terminal = TerminalInstance::new(
            self.terminal_size.0,
            self.terminal_size.1,
            &cwd,
            wakeup,
        );
        self.terminal = Some(terminal);
        tracing::info!(
            cols = self.terminal_size.0,
            rows = self.terminal_size.1,
            "Terminal spawned"
        );
    }

    fn setup_cursor_blink(&self) {
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));
            loop {
                interval.tick().await;
                if proxy.send_event(AppEvent::CursorBlink).is_err() {
                    break;
                }
            }
        });
    }

    fn calc_terminal_size(
        &self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> (u16, u16) {
        // Reserve space for egui header (~60px)
        let header_height = 60.0_f32;
        let available_height = (height - header_height).max(cell_height);
        let cols = (width / cell_width).floor() as u16;
        let rows = (available_height / cell_height).floor() as u16;
        (cols.max(2), rows.max(1))
    }

    fn handle_resize(&mut self, width: u32, height: u32) {
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(width, height);
        }

        // Get cell dimensions first to avoid borrow conflict
        let cell_dims = self
            .terminal_renderer
            .as_ref()
            .map(|r| (r.cell_width, r.cell_height));

        if let Some((cw, ch)) = cell_dims {
            let (cols, rows) = self.calc_terminal_size(width as f32, height as f32, cw, ch);
            if (cols, rows) != self.terminal_size {
                self.terminal_size = (cols, rows);
                if let Some(terminal) = &mut self.terminal {
                    terminal.resize(cols, rows);
                    tracing::debug!(cols, rows, "Terminal resized");
                }
            }
        }
    }

    fn render(&mut self) {
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

        // Process terminal events
        if let Some(terminal) = &mut self.terminal {
            terminal.process_events();
        }

        // Run egui
        let egui_state = self.egui_state.as_mut().unwrap();
        let raw_input = egui_state.take_egui_input(&gpu.window);
        let has_terminal = self.terminal.is_some();
        let term_size = self.terminal_size;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            Self::draw_ui_static(ctx, has_terminal, term_size);
        });

        egui_state.handle_platform_output(&gpu.window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

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

            // Render egui first (menu/header panels with opaque frames)
            egui_renderer.render(
                &mut render_pass,
                &paint_jobs,
                &screen_descriptor,
            );

            // Render terminal text on top
            if let Some(renderer) = &self.terminal_renderer {
                renderer.render_pass(&mut render_pass);
            }
        }

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    fn draw_ui_static(ctx: &egui::Context, has_terminal: bool, term_size: (u16, u16)) {
        let panel_frame = egui::Frame::new().fill(egui::Color32::from_rgb(37, 37, 38));
        egui::TopBottomPanel::top("menu_bar")
            .frame(panel_frame)
            .show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open Project...").clicked() {
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Toggle Theme").clicked() {
                        ui.close_menu();
                    }
                });
            });
        });

        egui::TopBottomPanel::top("header")
            .frame(panel_frame)
            .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VibeTreeRS");
                ui.separator();
                if has_terminal {
                    ui.label(format!("Terminal {}x{}", term_size.0, term_size.1));
                } else {
                    ui.label("No terminal");
                }
            });
        });

        // Central panel — transparent fill so GPU-rendered terminal text shows through
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
            .show(ctx, |_ui| {});
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
                self.spawn_terminal();
                self.setup_cursor_blink();
            }
            Err(e) => {
                tracing::error!("Failed to create window: {}", e);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Let egui handle the event first
        if let Some(egui_state) = &mut self.egui_state {
            if let Some(gpu) = &self.gpu {
                let response = egui_state.on_window_event(&gpu.window, &event);
                if response.consumed {
                    if response.repaint {
                        gpu.window.request_redraw();
                    }
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("Window close requested");
                self.terminal.take();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.handle_resize(size.width, size.height);
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Prepare terminal content for rendering
                if let (Some(terminal), Some(renderer), Some(gpu)) = (
                    &self.terminal,
                    &mut self.terminal_renderer,
                    &self.gpu,
                ) {
                    renderer.prepare(
                        &terminal.term,
                        &gpu.device,
                        &gpu.queue,
                        gpu.config.width,
                        gpu.config.height,
                        0.0,
                        60.0, // below egui header
                    );
                }
                self.render();
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
                if let Some(terminal) = &self.terminal {
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
                        _ => {
                            if let Some(text) = text {
                                terminal.write(text.as_bytes());
                            }
                        }
                    }
                }
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Redraw => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            AppEvent::PtyOutput { .. } => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            AppEvent::PtyExited { session_id, code } => {
                tracing::info!(session_id, code, "PTY exited");
            }
            AppEvent::CursorBlink => {
                // Toggle cursor visibility and redraw
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        }
    }
}
