use crate::event::AppEvent;
use crate::gpu::GpuContext;
use std::sync::Arc;
use vt_core::config::AppConfig;
use vt_core::types::Theme;
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
        let terminal = TerminalInstance::new(80, 24, &cwd);
        self.terminal = Some(terminal);
        tracing::info!("Terminal spawned");
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
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            Self::draw_ui_static(ctx, has_terminal);
        });

        egui_state.handle_platform_output(&gpu.window, full_output.platform_output);

        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let egui_renderer = self.egui_renderer.as_mut().unwrap();

        // Update egui textures
        for (id, delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });

        // Update egui buffers
        let _cmds = egui_renderer.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        // Render pass
        let bg = &self.theme_colors.terminal_bg;
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

            // Render terminal text
            if let Some(renderer) = &self.terminal_renderer {
                renderer.render_pass(&mut render_pass);
            }

            // Render egui on top — requires 'static lifetime render pass
            // This is safe: we drop the render_pass before accessing encoder again
            egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    fn draw_ui_static(ctx: &egui::Context, has_terminal: bool) {
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
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

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VibeTree");
                ui.separator();
                ui.label("Vibe code with AI in parallel git worktrees");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if has_terminal {
                ui.label("Terminal active — GPU-rendered text below");
                ui.label("Type to interact with the shell");
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("No terminal active.");
                });
            }
        });
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("VibeTree")
            .with_inner_size(winit::dpi::LogicalSize::new(1200, 800));

        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                self.initialize_gpu(window);
                self.spawn_terminal();
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
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
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
                        60.0,
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
            AppEvent::PtyOutput { .. } | AppEvent::Redraw => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
            AppEvent::PtyExited { session_id, code } => {
                tracing::info!(session_id, code, "PTY exited");
            }
            AppEvent::CursorBlink => {
                if let Some(gpu) = &self.gpu {
                    gpu.window.request_redraw();
                }
            }
        }
    }
}
