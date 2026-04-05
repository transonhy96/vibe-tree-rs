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
}

impl TerminalRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        font_size: f32,
    ) -> Self {
        let font_system = FontSystem::new();
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

        // Approximate monospace cell dimensions
        let cell_width = font_size * 0.6;
        let cell_height = font_size * 1.2;

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
        }
    }

    /// Prepare terminal content for rendering. Call before render_pass().
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
        let cols = term.columns();

        let metrics = Metrics::new(self.font_size, self.cell_height);

        // Build one text line per terminal row for efficient rendering.
        // Collect row data: (line_index, chars_with_colors)
        let mut row_data: Vec<(i32, Vec<(char, GlyphonColor)>)> = Vec::new();
        let mut current_line: i32 = i32::MIN;
        let mut current_chars: Vec<(char, GlyphonColor)> = Vec::new();

        for indexed in content.display_iter {
            let line = indexed.point.line.0;
            let col = indexed.point.column.0;

            if line != current_line {
                if current_line != i32::MIN && !current_chars.is_empty() {
                    row_data.push((current_line, std::mem::take(&mut current_chars)));
                }
                current_line = line;
                current_chars.clear();
            }

            // Pad with spaces if there are gaps
            while current_chars.len() < col {
                current_chars.push((' ', GlyphonColor::rgba(0, 0, 0, 0)));
            }

            let fg = self.resolve_color(indexed.cell.fg);
            current_chars.push((indexed.cell.c, fg));
        }
        if current_line != i32::MIN && !current_chars.is_empty() {
            row_data.push((current_line, current_chars));
        }

        drop(term);

        // Build glyphon buffers — one per row
        let mut buffers: Vec<GlyphonBuffer> = Vec::with_capacity(row_data.len());
        let mut positions: Vec<(f32, f32, GlyphonColor)> = Vec::with_capacity(row_data.len());

        for (line, chars) in &row_data {
            let y = offset_y + (*line as f32 + screen_lines as f32) * self.cell_height;
            let x = offset_x;

            // Build the line text and find the dominant color (use first non-space char's color)
            let text: String = chars.iter().map(|(c, _)| c).collect();
            let default_color = chars
                .iter()
                .find(|(c, _)| *c != ' ' && *c != '\0')
                .map(|(_, color)| *color)
                .unwrap_or(GlyphonColor::rgb(211, 215, 207));

            let mut buf = GlyphonBuffer::new(&mut self.font_system, metrics);
            buf.set_text(
                &mut self.font_system,
                &text,
                Attrs::new().family(Family::Monospace).color(default_color),
                Shaping::Basic,
            );
            buf.shape_until_scroll(&mut self.font_system, false);

            positions.push((x, y, default_color));
            buffers.push(buf);
        }

        // Build text areas from buffers
        let text_areas: Vec<TextArea<'_>> = buffers
            .iter()
            .zip(positions.iter())
            .map(|(buf, (x, y, color))| {
                let width = cols as f32 * self.cell_width;
                TextArea {
                    buffer: buf,
                    left: *x,
                    top: *y,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: *x as i32,
                        top: *y as i32,
                        right: (*x + width) as i32,
                        bottom: (*y + self.cell_height) as i32,
                    },
                    default_color: *color,
                    custom_glyphs: &[],
                }
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

    fn resolve_color(&self, color: TermColor) -> GlyphonColor {
        match color {
            TermColor::Named(named) => named_to_rgb(named),
            TermColor::Spec(rgb) => GlyphonColor::rgb(rgb.r, rgb.g, rgb.b),
            TermColor::Indexed(idx) => indexed_to_rgb(idx),
        }
    }

    /// Render the prepared text into the given render pass.
    pub fn render_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let _ = self
            .text_renderer
            .render(&self.text_atlas, &self.viewport, render_pass);
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
