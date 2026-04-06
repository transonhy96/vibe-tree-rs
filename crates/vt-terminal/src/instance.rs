use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Waker function type — called when the terminal has new content to display.
pub type WakeupFn = Arc<dyn Fn() + Send + Sync>;

/// Forwards terminal events to a channel and wakes the render loop.
#[derive(Clone)]
pub struct EventProxy {
    sender: std::sync::mpsc::Sender<TermEvent>,
    wakeup: WakeupFn,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        let _ = self.sender.send(event);
        (self.wakeup)();
    }
}

pub struct TerminalInstance {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: EventLoopSender,
    pub dirty: bool,
    event_rx: std::sync::mpsc::Receiver<TermEvent>,
    _pty_thread: JoinHandle<()>,
    selection_start: std::sync::Mutex<Option<(usize, i32)>>,
}

impl TerminalInstance {
    pub fn new(cols: u16, rows: u16, cwd: &Path, wakeup: WakeupFn) -> Self {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let event_proxy = EventProxy {
            sender: event_tx,
            wakeup,
        };

        let term_size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(term::Config::default(), &term_size, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let window_size = WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: 1,
            cell_height: 1,
        };

        let shell = Self::default_shell();
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());

        let pty_config = tty::Options {
            shell: Some(tty::Shell::new(shell, Vec::new())),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
        };

        let pty = tty::new(&pty_config, window_size, 0).expect("failed to create PTY");

        let event_loop =
            PtyEventLoop::new(term.clone(), event_proxy, pty, pty_config.drain_on_exit, false)
                .expect("failed to create event loop");

        let notifier = event_loop.channel();

        let pty_thread = std::thread::spawn(move || {
            let _ = event_loop.spawn().join();
        });

        Self {
            term,
            notifier,
            dirty: true,
            event_rx,
            _pty_thread: pty_thread,
            selection_start: std::sync::Mutex::new(None),
        }
    }

    /// Process any pending terminal events (title changes, etc.)
    pub fn process_events(&mut self) {
        while let Ok(_event) = self.event_rx.try_recv() {
            self.dirty = true;
        }
    }

    /// Send input bytes to the PTY.
    pub fn write(&self, data: &[u8]) {
        let _ = self.notifier.send(Msg::Input(Cow::Owned(data.to_vec())));
    }

    /// Get visible terminal text as a string (for scanning/detection).
    /// Returns trimmed lines only (no trailing spaces).
    pub fn visible_text(&self) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut lines: Vec<String> = Vec::new();
        let mut current_line: i32 = i32::MIN;
        let mut current_chars = String::new();

        for indexed in content.display_iter {
            let line = indexed.point.line.0;
            if line != current_line {
                if current_line != i32::MIN {
                    let trimmed = current_chars.trim_end().to_string();
                    if !trimmed.is_empty() {
                        lines.push(trimmed);
                    }
                    current_chars.clear();
                }
                current_line = line;
            }
            current_chars.push(indexed.cell.c);
        }
        if !current_chars.is_empty() {
            let trimmed = current_chars.trim_end().to_string();
            if !trimmed.is_empty() {
                lines.push(trimmed);
            }
        }
        lines.join("\n")
    }

    /// Start a text selection at the given terminal grid position.
    pub fn start_selection(&self, col: usize, line: i32) {
        let point = Point::new(Line(line), Column(col));
        let mut term = self.term.lock();
        term.selection = Some(Selection::new(SelectionType::Simple, point, Direction::Left));
        *self.selection_start.lock().unwrap() = Some((col, line));
    }

    /// Update the selection to extend to the given position.
    pub fn update_selection(&self, col: usize, line: i32) {
        let point = Point::new(Line(line), Column(col));
        let mut term = self.term.lock();
        if let Some(ref mut sel) = term.selection {
            // Use Left side when dragging left/up, Right when dragging right/down
            let start = self.selection_start.lock().unwrap();
            let side = if let Some((sc, sl)) = *start {
                if line < sl || (line == sl && col <= sc) {
                    Direction::Left
                } else {
                    Direction::Right
                }
            } else {
                Direction::Right
            };
            sel.update(point, side);
        }
    }

    /// Clear any active selection.
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        term.selection = None;
    }

    /// Get the selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        let term = self.term.lock();
        term.selection_to_string()
    }

    /// Scroll the terminal display.
    pub fn scroll(&self, delta: i32) {
        self.term.lock().scroll_display(Scroll::Delta(delta));
    }

    /// Resize the terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let window_size = WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: 1,
            cell_height: 1,
        };
        let _ = self.notifier.send(Msg::Resize(window_size));

        let term_size = TermSize::new(cols as usize, rows as usize);
        self.term.lock().resize(term_size);
        self.dirty = true;
    }

    fn default_shell() -> String {
        if cfg!(target_os = "windows") {
            "powershell.exe".to_string()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
        }
    }
}

impl Drop for TerminalInstance {
    fn drop(&mut self) {
        let _ = self.notifier.send(Msg::Shutdown);
    }
}
