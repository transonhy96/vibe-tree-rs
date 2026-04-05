use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use glyphon::{
    Attrs, Buffer as GlyphonBuffer, Cache, Color as GlyphonColor, Family, FontSystem, Metrics,
    Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use std::sync::Arc;
use wgpu;

use crate::instance::EventProxy;

/// Cached line data to avoid re-shaping text every frame.
struct CachedLine {
    buffer: GlyphonBuffer,
    x: f32,
    y: f32,
    color: GlyphonColor,
}

pub struct TerminalRenderer {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub text_atlas: TextAtlas,
    pub text_renderer: TextRenderer,
    pub viewport: Viewport,
    _cache: Cache,
    pub cell_width: f32,
    pub cell_height: f32,
    font_size: f32,
    /// Cached shaped lines — only rebuilt when content changes.
    cached_lines: Vec<CachedLine>,
    /// Hash of last rendered content to detect changes.
    last_content_hash: u64,
}

impl TerminalRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        font_size: f32,
    ) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut text_atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );
        let viewport = Viewport::new(device, &cache);

        let metrics = Metrics::new(font_size, font_size * 1.2);
        let mut measure_buf = GlyphonBuffer::new(&mut font_system, metrics);
        measure_buf.set_text(
            &mut font_system,
            "MMMMMMMMMM",
            Attrs::new().family(Family::Monospace),
            Shaping::Basic,
        );
        measure_buf.shape_until_scroll(&mut font_system, false);
        let cell_width = measure_buf
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first().map(|g| g.w))
            .unwrap_or(font_size * 0.6);
        let cell_height = font_size * 1.2;

        tracing::info!(cell_width, cell_height, font_size, "Font metrics measured");

        Self {
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            viewport,
            _cache: cache,
            cell_width,
            cell_height,
            font_size,
            cached_lines: Vec::new(),
            last_content_hash: 0,
        }
    }

    pub fn prepare(
        &mut self,
        term: &Arc<FairMutex<Term<EventProxy>>>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_width: u32,
        screen_height: u32,
        offset_x: f32,
        offset_y: f32,
    ) {
        self.viewport.update(
            queue,
            Resolution {
                width: screen_width,
                height: screen_height,
            },
        );

        let term = term.lock();
        let content = term.renderable_content();
        let screen_lines = term.screen_lines();

        // Build a quick hash of visible content to detect changes
        let mut content_hash: u64 = 0;
        let mut row_data: Vec<(i32, String, GlyphonColor)> = Vec::new();
        let mut current_line: i32 = i32::MIN;
        let mut current_chars: Vec<(char, GlyphonColor)> = Vec::new();

        for indexed in content.display_iter {
            let line = indexed.point.line.0;
            let col = indexed.point.column.0;
            let c = indexed.cell.c;

            // Simple hash
            content_hash = content_hash
                .wrapping_mul(31)
                .wrapping_add(c as u64)
                .wrapping_add(col as u64 * 97)
                .wrapping_add(line as u64 * 7919);

            if line != current_line {
                if current_line != i32::MIN {
                    Self::push_row(&mut row_data, current_line, &current_chars);
                }
                current_line = line;
                current_chars.clear();
            }

            while current_chars.len() < col {
                current_chars.push((' ', GlyphonColor::rgba(0, 0, 0, 0)));
            }

            let fg = resolve_color(indexed.cell.fg);
            current_chars.push((c, fg));
        }
        if current_line != i32::MIN {
            Self::push_row(&mut row_data, current_line, &current_chars);
        }

        // Also hash cursor position
        let cursor = content.cursor;
        content_hash = content_hash
            .wrapping_mul(31)
            .wrapping_add(cursor.point.line.0 as u64 * 13)
            .wrapping_add(cursor.point.column.0 as u64 * 17);

        drop(term);

        // Only rebuild buffers if content changed
        if content_hash != self.last_content_hash {
            self.last_content_hash = content_hash;
            self.rebuild_lines(&row_data, screen_lines, screen_width, screen_height, offset_x, offset_y);
        }

        // Prepare text areas from cache
        let text_areas: Vec<TextArea<'_>> = self
            .cached_lines
            .iter()
            .map(|cl| TextArea {
                buffer: &cl.buffer,
                left: cl.x,
                top: cl.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: screen_width as i32,
                    bottom: screen_height as i32,
                },
                default_color: cl.color,
                custom_glyphs: &[],
            })
            .collect();

        let _ = self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.text_atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );
    }

    fn rebuild_lines(
        &mut self,
        row_data: &[(i32, String, GlyphonColor)],
        screen_lines: usize,
        screen_width: u32,
        screen_height: u32,
        offset_x: f32,
        offset_y: f32,
    ) {
        let metrics = Metrics::new(self.font_size, self.cell_height);

        let last_line = row_data.last().map(|(l, _, _)| *l).unwrap_or(0);
        let first_line = row_data.first().map(|(l, _, _)| *l).unwrap_or(0);
        let content_lines = (last_line - first_line + 1) as f32;
        let available_rows = ((screen_height as f32 - offset_y) / self.cell_height).floor();

        let y_base = if content_lines < available_rows {
            offset_y - first_line as f32 * self.cell_height
        } else {
            offset_y + screen_lines as f32 * self.cell_height
        };

        self.cached_lines.clear();

        for (line, text, color) in row_data {
            let y = y_base + *line as f32 * self.cell_height;
            let x = offset_x;

            let mut buf = GlyphonBuffer::new(&mut self.font_system, metrics);
            buf.set_size(&mut self.font_system, Some(screen_width as f32), None);
            buf.set_text(
                &mut self.font_system,
                text,
                Attrs::new().family(Family::Monospace).color(*color),
                Shaping::Basic,
            );
            buf.shape_until_scroll(&mut self.font_system, false);

            self.cached_lines.push(CachedLine {
                buffer: buf,
                x,
                y,
                color: *color,
            });
        }
    }

    fn push_row(
        row_data: &mut Vec<(i32, String, GlyphonColor)>,
        line: i32,
        chars: &[(char, GlyphonColor)],
    ) {
        let has_content = chars.iter().any(|(c, _)| !c.is_whitespace() && *c != '\0');
        if !has_content {
            return;
        }
        let text: String = chars.iter().map(|(c, _)| c).collect();
        let color = chars
            .iter()
            .find(|(c, _)| !c.is_whitespace() && *c != '\0')
            .map(|(_, color)| *color)
            .unwrap_or(GlyphonColor::rgb(211, 215, 207));
        row_data.push((line, text, color));
    }

    pub fn render_pass(&self, render_pass: &mut wgpu::RenderPass<'static>) {
        let _ = self
            .text_renderer
            .render(&self.text_atlas, &self.viewport, render_pass);
    }
}

fn resolve_color(color: TermColor) -> GlyphonColor {
    match color {
        TermColor::Named(named) => named_to_rgb(named),
        TermColor::Spec(rgb) => GlyphonColor::rgb(rgb.r, rgb.g, rgb.b),
        TermColor::Indexed(idx) => indexed_to_rgb(idx),
    }
}

fn named_to_rgb(color: NamedColor) -> GlyphonColor {
    match color {
        NamedColor::Black => GlyphonColor::rgb(0, 0, 0),
        NamedColor::Red => GlyphonColor::rgb(204, 0, 0),
        NamedColor::Green => GlyphonColor::rgb(78, 154, 6),
        NamedColor::Yellow => GlyphonColor::rgb(196, 160, 0),
        NamedColor::Blue => GlyphonColor::rgb(52, 101, 164),
        NamedColor::Magenta => GlyphonColor::rgb(117, 80, 123),
        NamedColor::Cyan => GlyphonColor::rgb(6, 152, 154),
        NamedColor::White => GlyphonColor::rgb(211, 215, 207),
        NamedColor::BrightBlack => GlyphonColor::rgb(85, 87, 83),
        NamedColor::BrightRed => GlyphonColor::rgb(239, 41, 41),
        NamedColor::BrightGreen => GlyphonColor::rgb(138, 226, 52),
        NamedColor::BrightYellow => GlyphonColor::rgb(252, 233, 79),
        NamedColor::BrightBlue => GlyphonColor::rgb(114, 159, 207),
        NamedColor::BrightMagenta => GlyphonColor::rgb(173, 127, 168),
        NamedColor::BrightCyan => GlyphonColor::rgb(52, 226, 226),
        NamedColor::BrightWhite => GlyphonColor::rgb(238, 238, 236),
        NamedColor::Foreground => GlyphonColor::rgb(211, 215, 207),
        NamedColor::Background => GlyphonColor::rgb(30, 30, 30),
        _ => GlyphonColor::rgb(211, 215, 207),
    }
}

fn indexed_to_rgb(idx: u8) -> GlyphonColor {
    if idx < 16 {
        let named = match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            15 => NamedColor::BrightWhite,
            _ => unreachable!(),
        };
        named_to_rgb(named)
    } else if idx < 232 {
        let idx = idx - 16;
        let r = (idx / 36) * 51;
        let g = ((idx % 36) / 6) * 51;
        let b = (idx % 6) * 51;
        GlyphonColor::rgb(r, g, b)
    } else {
        let val = 8 + (idx - 232) * 10;
        GlyphonColor::rgb(val, val, val)
    }
}
