#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(target_os = "linux")]
pub fn x11_backend_new() -> Result<x11::X11Backend, String> {
    x11::X11Backend::new()
}

#[cfg(not(target_os = "linux"))]
pub fn x11_backend_new() -> Result<(), String> {
    Err("Not supported".into())
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("window not found for pid {0}")]
    WindowNotFound(u32),
    #[error("embed failed: {0}")]
    Failed(String),
    #[error("platform not supported")]
    Unsupported,
}

/// Rect in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmbedRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Handle to an embedded window. Drop to release.
pub struct EmbeddedWindow {
    pub child_id: u64,
    pub original_parent: u64,
    pub parent_window_id: u64,
    pub portal_rect: EmbedRect,
    #[cfg(target_os = "linux")]
    pub backend: x11::X11Backend,
    pub overlay_mode: bool,
}

impl EmbeddedWindow {
    /// Reposition/resize the embedded window.
    pub fn set_bounds(&self, rect: EmbedRect) -> Result<(), EmbedError> {
        #[cfg(target_os = "linux")]
        {
            if self.overlay_mode {
                // Get parent position and compute absolute coords
                let parent_pos = self.backend.get_window_position(self.parent_window_id);
                let abs_rect = EmbedRect {
                    x: parent_pos.0 + rect.x,
                    y: parent_pos.1 + rect.y,
                    width: rect.width,
                    height: rect.height,
                };
                self.backend.set_bounds(self.child_id, abs_rect)?;
                self.backend.raise_window(self.child_id)?;
                Ok(())
            } else {
                self.backend.set_bounds(self.child_id, rect)
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = rect;
            Err(EmbedError::Unsupported)
        }
    }
}

impl Drop for EmbeddedWindow {
    fn drop(&mut self) {
        // No cleanup needed for overlay mode — window stays where it is
    }
}

/// Find a window belonging to a process and embed it into a parent window.
pub fn embed_window_by_pid(
    parent_window_id: u64,
    pid: u32,
    rect: EmbedRect,
) -> Result<EmbeddedWindow, EmbedError> {
    #[cfg(target_os = "linux")]
    {
        let backend = x11::X11Backend::new()
            .map_err(|e| EmbedError::Failed(e.to_string()))?;

        let child_id = backend.find_window_by_pid(pid)?;
        let root = backend.root_window();

        let parent_pos = backend.get_window_position(parent_window_id);
        let abs_rect = EmbedRect {
            x: parent_pos.0 + rect.x,
            y: parent_pos.1 + rect.y,
            width: rect.width,
            height: rect.height,
        };
        backend.set_bounds(child_id, abs_rect)?;
        backend.raise_window(child_id)?;

        tracing::info!(child_id, parent_window_id, pid, "Window overlaid");

        Ok(EmbeddedWindow {
            child_id,
            original_parent: root,
            parent_window_id,
            portal_rect: rect,
            backend,
            overlay_mode: true,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent_window_id, pid, rect);
        Err(EmbedError::Unsupported)
    }
}

/// Find a window by its name/title substring and embed it.
/// Uses overlay mode (move/resize without reparent) for complex apps.
pub fn embed_window_by_name(
    parent_window_id: u64,
    name: &str,
    rect: EmbedRect,
) -> Result<EmbeddedWindow, EmbedError> {
    #[cfg(target_os = "linux")]
    {
        let backend = x11::X11Backend::new()
            .map_err(|e| EmbedError::Failed(e.to_string()))?;

        let child_id = backend.find_window_by_name(name)?;
        let root = backend.root_window();

        // Get parent window position on screen for absolute positioning
        let parent_pos = backend.get_window_position(parent_window_id);

        // Position child window over the portal area (no reparent)
        let abs_rect = EmbedRect {
            x: parent_pos.0 + rect.x,
            y: parent_pos.1 + rect.y,
            width: rect.width,
            height: rect.height,
        };
        backend.set_bounds(child_id, abs_rect)?;
        backend.raise_window(child_id)?;

        tracing::info!(child_id, name, "Window overlaid (no reparent)");

        Ok(EmbeddedWindow {
            child_id,
            original_parent: root,
            parent_window_id,
            portal_rect: rect,
            backend,
            overlay_mode: true,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent_window_id, name, rect);
        Err(EmbedError::Unsupported)
    }
}
