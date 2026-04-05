#[cfg(target_os = "linux")]
mod x11;

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
    #[cfg(target_os = "linux")]
    backend: x11::X11Backend,
}

impl EmbeddedWindow {
    /// Reposition/resize the embedded window within the parent.
    pub fn set_bounds(&self, rect: EmbedRect) -> Result<(), EmbedError> {
        #[cfg(target_os = "linux")]
        {
            self.backend.set_bounds(self.child_id, rect)
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
        // Reparent back to original parent on drop
        #[cfg(target_os = "linux")]
        {
            let _ = self.backend.reparent(self.child_id, self.original_parent, 0, 0);
        }
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

        // Reparent into our window
        backend.reparent(child_id, parent_window_id, rect.x, rect.y)?;
        backend.set_bounds(child_id, rect)?;
        backend.map_window(child_id)?;

        tracing::info!(child_id, parent_window_id, pid, "Window embedded");

        Ok(EmbeddedWindow {
            child_id,
            original_parent: root,
            backend,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent_window_id, pid, rect);
        Err(EmbedError::Unsupported)
    }
}

/// Find a window by its name/title substring and embed it.
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

        backend.reparent(child_id, parent_window_id, rect.x, rect.y)?;
        backend.set_bounds(child_id, rect)?;
        backend.map_window(child_id)?;

        tracing::info!(child_id, name, "Window embedded by name");

        Ok(EmbeddedWindow {
            child_id,
            original_parent: root,
            backend,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent_window_id, name, rect);
        Err(EmbedError::Unsupported)
    }
}
