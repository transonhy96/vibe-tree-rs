use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use glyphon::{
    Attrs, Buffer as GlyphonBuffer, Cache, Color as GlyphonColor, Family, FontSystem, Metrics,
    Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use regex::Regex;
use std::sync::Arc;
use wgpu;

use crate::instance::EventProxy;

/// A detected URL in the terminal with its position.
#[derive(Clone, Debug)]
pub struct DetectedUrl {
    pub url: String,
    /// Terminal grid line index (from display_iter)
    pub line: i32,
    /// Start column (inclusive)
    pub col_start: usize,
    /// End column (exclusive)
    pub col_end: usize,
}

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
    cached_lines: Vec<CachedLine>,
    pub last_content_hash: u64,
    cursor_pos: Option<(f32, f32)>,
    cursor_blink_visible: bool,
    cursor_active: bool,
    last_input_time: std::time::Instant,
    cursor_buffer: Option<GlyphonBuffer>,
    /// Divider line buffer (shown when scrolled)
    divider_buffer: Option<CachedLine>,
    /// Detected URLs in visible terminal content.
    pub detected_urls: Vec<DetectedUrl>,
    /// Underline buffers for URLs.
    url_underlines: Vec<CachedLine>,
    url_regex: Regex,
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
            cursor_pos: None,
            cursor_blink_visible: true,
            cursor_active: false,
            last_input_time: std::time::Instant::now(),
            cursor_buffer: None,
            divider_buffer: None,
            detected_urls: Vec::new(),
            url_underlines: Vec::new(),
            url_regex: Regex::new(r#"https?://[^\s<>"'`\]\)]+"#).unwrap(),
        }
    }

    pub fn mark_input(&mut self) {
        self.cursor_active = true;
        self.cursor_blink_visible = true;
    }

    pub fn toggle_cursor_blink(&mut self) -> bool {
        self.cursor_blink_visible = !self.cursor_blink_visible;
        true
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
        let cols = term.columns();
        let display_offset = term.grid().display_offset();
        let selection = content.selection.clone();

        // Build content hash and row data from display_iter (scrollback view)
        let mut content_hash: u64 = 0;
        let mut row_data: Vec<(i32, String, GlyphonColor)> = Vec::new();
        let mut current_line: i32 = i32::MIN;
        let mut current_chars: Vec<(char, GlyphonColor)> = Vec::new();

        // Hash selection state
        if let Some(ref sel) = selection {
            content_hash = content_hash
                .wrapping_mul(31)
                .wrapping_add((sel.start.line.0 as i64 as u64).wrapping_mul(101))
                .wrapping_add((sel.start.column.0 as u64).wrapping_mul(103))
                .wrapping_add((sel.end.line.0 as i64 as u64).wrapping_mul(107))
                .wrapping_add((sel.end.column.0 as u64).wrapping_mul(109));
        }

        for indexed in content.display_iter {
            let line = indexed.point.line.0;
            let col = indexed.point.column.0;
            let c = indexed.cell.c;

            content_hash = content_hash
                .wrapping_mul(31)
                .wrapping_add(c as u64)
                .wrapping_add(col as u64 * 97)
                .wrapping_add((line as i64 as u64).wrapping_mul(7919));

            if line != current_line {
                if current_line != i32::MIN {
                    Self::push_row(&mut row_data, current_line, &current_chars, &selection);
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
            Self::push_row(&mut row_data, current_line, &current_chars, &selection);
        }

        // If scrolled, also read the live bottom lines directly from grid
        let mut live_rows: Vec<(i32, String, GlyphonColor)> = Vec::new();
        let is_scrolled = display_offset > 0;
        let live_line_count = if is_scrolled {
            20_usize
        } else {
            0
        };

        if is_scrolled {
            // Read the bottom N lines from grid at ACTUAL terminal position.
            // Grid indexing: Line(0) = top of visible area, Line(screen_lines-1) = bottom.
            // The bottom line (prompt) is at Line(screen_lines-1).
            // Read bottom live_line_count lines: Line(screen_lines-live_count) to Line(screen_lines-1)
            let bottom = screen_lines as i32 - 1;
            for i in 0..live_line_count {
                let line_idx = Line(bottom - (live_line_count as i32 - 1 - i as i32));
                let mut chars: Vec<(char, GlyphonColor)> = Vec::new();
                for col_idx in 0..cols {
                    let point = Point::new(line_idx, Column(col_idx));
                    let cell = &term.grid()[point];
                    let fg = resolve_color(cell.fg);
                    chars.push((cell.c, fg));
                }
                Self::push_row(&mut live_rows, i as i32, &chars, &None);
            }

            // Hash live rows too
            for (_, text, _) in &live_rows {
                for c in text.chars() {
                    content_hash = content_hash.wrapping_mul(31).wrapping_add(c as u64);
                }
            }
        }

        // Cursor
        let cursor = content.cursor;
        let blink_bit = if self.cursor_blink_visible { 1u64 } else { 0 };
        content_hash = content_hash
            .wrapping_mul(31)
            .wrapping_add((cursor.point.line.0 as i64 as u64).wrapping_mul(13))
            .wrapping_add(cursor.point.column.0 as u64 * 17)
            .wrapping_add(blink_bit);

        let cursor_line = cursor.point.line.0;
        let cursor_col = cursor.point.column.0;

        drop(term);

        // Detect URLs before rebuild (modifies row_data colors)
        let needs_rebuild = content_hash != self.last_content_hash || is_scrolled;
        if needs_rebuild {
            self.detected_urls.clear();
            for (line, text, _) in &row_data {
                for mat in self.url_regex.find_iter(text) {
                    self.detected_urls.push(DetectedUrl {
                        url: mat.as_str().to_string(),
                        line: *line,
                        col_start: mat.start(),
                        col_end: mat.end(),
                    });
                }
            }
        }

        if needs_rebuild {
            self.last_content_hash = content_hash;

            let available_height = screen_height as f32 - offset_y;

            if is_scrolled {
                // Split view: scrollback at top, divider, live at bottom.
                // All positioned sequentially from offset_y using simple row counting.
                let actual_live = live_line_count.min(screen_lines * 2 / 3).max(2);
                let scrollback_display_rows = screen_lines - actual_live - 1;

                // Scrollback: place rows sequentially from top.
                // Each row_data entry maps to a sequential screen row.
                let metrics = Metrics::new(self.font_size, self.cell_height);
                self.cached_lines.clear();
                let mut screen_row = 0usize;
                for (_, text, color) in &row_data {
                    if screen_row >= scrollback_display_rows { break; }
                    let has_content = text.chars().any(|c| !c.is_whitespace() && c != '\0');
                    if !has_content {
                        screen_row += 1;
                        continue;
                    }
                    let y = offset_y + screen_row as f32 * self.cell_height;
                    let mut buf = GlyphonBuffer::new(&mut self.font_system, metrics);
                    buf.set_size(&mut self.font_system, Some(screen_width as f32), None);
                    buf.set_text(&mut self.font_system, text,
                        Attrs::new().family(Family::Monospace).color(*color), Shaping::Basic);
                    buf.shape_until_scroll(&mut self.font_system, false);
                    self.cached_lines.push(CachedLine { buffer: buf, x: offset_x, y, color: *color });
                    screen_row += 1;
                }

                // Divider
                let divider_y = offset_y + scrollback_display_rows as f32 * self.cell_height;
                let metrics = Metrics::new(self.font_size, self.cell_height);
                let label = " LIVE ";
                let side_len = cols.saturating_sub(label.len()) / 2;
                let divider_text = format!(
                    "{}{}{}",
                    "-".repeat(side_len),
                    label,
                    "-".repeat(cols.saturating_sub(side_len + label.len()))
                );
                let mut div_buf = GlyphonBuffer::new(&mut self.font_system, metrics);
                div_buf.set_size(&mut self.font_system, Some(screen_width as f32), None);
                div_buf.set_text(
                    &mut self.font_system, &divider_text,
                    Attrs::new().family(Family::Monospace).color(GlyphonColor::rgb(100, 100, 100)),
                    Shaping::Basic,
                );
                div_buf.shape_until_scroll(&mut self.font_system, false);
                self.divider_buffer = Some(CachedLine {
                    buffer: div_buf, x: offset_x, y: divider_y,
                    color: GlyphonColor::rgb(100, 100, 100),
                });

                // Live section: placed right after divider
                let live_start_y = divider_y + self.cell_height;
                let live_metrics = Metrics::new(self.font_size, self.cell_height);
                for (i, (_, text, color)) in live_rows.iter().enumerate() {
                    if i >= actual_live { break; }
                    let has_content = text.chars().any(|c| !c.is_whitespace() && c != '\0');
                    if !has_content { continue; }
                    let y = live_start_y + i as f32 * self.cell_height;
                    let mut buf = GlyphonBuffer::new(&mut self.font_system, live_metrics);
                    buf.set_size(&mut self.font_system, Some(screen_width as f32), None);
                    buf.set_text(
                        &mut self.font_system, text,
                        Attrs::new().family(Family::Monospace).color(*color),
                        Shaping::Basic,
                    );
                    buf.shape_until_scroll(&mut self.font_system, false);
                    self.cached_lines.push(CachedLine {
                        buffer: buf, x: offset_x, y, color: *color,
                    });
                }
            } else {
                // Normal view: no split
                self.divider_buffer = None;

                // Find first non-empty line to shift content up when screen isn't full
                let first_content_line = row_data.iter()
                    .find(|(_, text, _)| text.chars().any(|c| !c.is_whitespace() && c != '\0'))
                    .map(|(l, _, _)| *l)
                    .unwrap_or(0);
                // Shift so first content line appears at top
                // first_content_line's absolute row = first_content_line + screen_lines - 1
                // We want that row at y=offset_y, so shift = -(row) * cell_height
                let content_row = (first_content_line + screen_lines as i32 - 1) as f32;
                let y_shift = -content_row * self.cell_height;

                self.rebuild_lines_with_shift(
                    &row_data,
                    screen_lines,
                    screen_width,
                    offset_x,
                    offset_y,
                    screen_lines,
                    y_shift,
                );
            }

            // Cursor
            let cx = offset_x + cursor_col as f32 * self.cell_width;
            let cy = if is_scrolled {
                // Cursor in live section
                let actual_live = live_line_count.min(screen_lines * 2 / 3).max(2);
                let scrollback_display_rows = screen_lines - actual_live - 1;
                let divider_y = offset_y + scrollback_display_rows as f32 * self.cell_height;
                let live_start_y = divider_y + self.cell_height;
                // cursor_line: 0=bottom (last live row), -(n-1)=top
                let row_in_live = (cursor_line + actual_live as i32 - 1).max(0) as f32;
                live_start_y + row_in_live * self.cell_height
            } else {
                // Normal: use same shift as rebuild
                let first_content_line = row_data.iter()
                    .find(|(_, text, _)| text.chars().any(|c| !c.is_whitespace() && c != '\0'))
                    .map(|(l, _, _)| *l)
                    .unwrap_or(0);
                let content_row = (first_content_line + screen_lines as i32 - 1) as f32;
                let y_shift = -content_row * self.cell_height;
                let row = (cursor_line + screen_lines as i32 - 1) as f32;
                offset_y + row * self.cell_height + y_shift
            };
            self.cursor_pos = Some((cx, cy));

            let show_cursor = self.cursor_blink_visible;
            if show_cursor {
                let metrics = Metrics::new(self.font_size, self.cell_height);
                let mut buf = GlyphonBuffer::new(&mut self.font_system, metrics);
                buf.set_size(&mut self.font_system, Some(self.cell_width * 2.0), None);
                buf.set_text(
                    &mut self.font_system,
                    "\u{2588}",
                    Attrs::new()
                        .family(Family::Monospace)
                        .color(GlyphonColor::rgba(200, 200, 200, 180)),
                    Shaping::Basic,
                );
                buf.shape_until_scroll(&mut self.font_system, false);
                self.cursor_buffer = Some(buf);
            } else {
                self.cursor_buffer = None;
            }
        }

        // Color lines containing URLs blue
        if needs_rebuild {
            self.url_underlines.clear();
            let url_color = GlyphonColor::rgb(100, 160, 255);
            for entry in &mut row_data {
                let has_url = self.detected_urls.iter().any(|u| u.line == entry.0);
                if has_url {
                    entry.2 = url_color;
                }
            }
        }

        // Build text areas from cache
        let mut text_areas: Vec<TextArea<'_>> = self
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

        // Add divider
        if let Some(div) = &self.divider_buffer {
            text_areas.push(TextArea {
                buffer: &div.buffer,
                left: div.x,
                top: div.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: screen_width as i32,
                    bottom: screen_height as i32,
                },
                default_color: div.color,
                custom_glyphs: &[],
            });
        }

        // Add cursor
        if let (Some((cx, cy)), Some(cursor_buf)) = (self.cursor_pos, &self.cursor_buffer) {
            text_areas.push(TextArea {
                buffer: cursor_buf,
                left: cx,
                top: cy,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: screen_width as i32,
                    bottom: screen_height as i32,
                },
                default_color: GlyphonColor::rgba(200, 200, 200, 180),
                custom_glyphs: &[],
            });
        }

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

    fn rebuild_lines_absolute(
        &mut self,
        row_data: &[(i32, String, GlyphonColor)],
        screen_lines: usize,
        screen_width: u32,
        offset_x: f32,
        offset_y: f32,
        max_rows: usize,
    ) {
        self.rebuild_lines_with_shift(row_data, screen_lines, screen_width, offset_x, offset_y, max_rows, 0.0);
    }

    /// Rebuild cached lines. y_shift moves all lines up (negative) or down.
    fn rebuild_lines_with_shift(
        &mut self,
        row_data: &[(i32, String, GlyphonColor)],
        screen_lines: usize,
        screen_width: u32,
        offset_x: f32,
        offset_y: f32,
        max_rows: usize,
        y_shift: f32,
    ) {
        let metrics = Metrics::new(self.font_size, self.cell_height);
        self.cached_lines.clear();

        for (_i, (line, text, color)) in row_data.iter().enumerate() {
            let row = (*line + screen_lines as i32 - 1) as f32;
            let y = offset_y + row * self.cell_height + y_shift;
            // Skip lines outside visible area (line must fit entirely)
            if y < offset_y || y + self.cell_height > offset_y + max_rows as f32 * self.cell_height {
                continue;
            }
            // Skip empty lines (no visible glyphs to shape)
            let has_content = text.chars().any(|c| !c.is_whitespace() && c != '\0');
            if !has_content {
                continue;
            }

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
                x: offset_x,
                y,
                color: *color,
            });
        }
    }

    fn push_row(
        row_data: &mut Vec<(i32, String, GlyphonColor)>,
        line: i32,
        chars: &[(char, GlyphonColor)],
        selection: &Option<alacritty_terminal::selection::SelectionRange>,
    ) {
        use alacritty_terminal::index::{Column, Line, Point};

        let text: String = chars.iter().map(|(c, _)| c).collect();

        // Check if any character in this line is selected
        let has_selection = selection.as_ref().map(|sel| {
            let matched = chars.iter().enumerate().any(|(col, _)| {
                sel.contains(Point::new(Line(line), Column(col)))
            });
            matched
        }).unwrap_or(false);

        let color = if has_selection {
            // Selected text: bright cyan for clear visual feedback
            GlyphonColor::rgb(0, 255, 255)
        } else {
            chars
                .iter()
                .find(|(c, _)| !c.is_whitespace() && *c != '\0')
                .map(|(_, color)| *color)
                .unwrap_or(GlyphonColor::rgb(211, 215, 207))
        };
        row_data.push((line, text, color));
    }

    /// Check if a terminal cell position (col, line) is on a detected URL.
    /// Returns the URL string if found.
    pub fn url_at_cell(&self, col: usize, line: i32) -> Option<&str> {
        self.detected_urls.iter().find(|u| {
            u.line == line && col >= u.col_start && col < u.col_end
        }).map(|u| u.url.as_str())
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
