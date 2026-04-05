/// Custom events sent to the winit event loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// PTY output received — terminal needs redraw.
    PtyOutput { session_id: u64 },
    /// PTY session exited.
    PtyExited { session_id: u64, code: i32 },
    /// Cursor blink timer tick.
    CursorBlink,
    /// Request a redraw.
    Redraw,
}
