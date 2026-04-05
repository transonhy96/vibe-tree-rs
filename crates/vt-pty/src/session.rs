use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    OpenFailed(String),
    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session not found: {0}")]
    NotFound(u64),
}

pub struct PtySession {
    pub id: u64,
    pub worktree_path: String,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn Write + Send>,
    reader: Option<Box<dyn Read + Send>>,
    master_pty: Box<dyn portable_pty::MasterPty + Send>,
}

impl PtySession {
    pub fn spawn(
        id: u64,
        worktree_path: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::OpenFailed(e.to_string()))?;

        let shell = Self::default_shell();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-l"); // Login shell
        cmd.cwd(worktree_path);

        // Set up environment
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::SpawnFailed(e.to_string()))?;

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        Ok(Self {
            id,
            worktree_path: worktree_path.to_string_lossy().to_string(),
            child,
            writer,
            reader: Some(reader),
            master_pty: pair.master,
        })
    }

    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.master_pty
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        Ok(())
    }

    pub fn try_wait(&mut self) -> Option<u32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.exit_code())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    fn default_shell() -> String {
        if cfg!(target_os = "windows") {
            "powershell.exe".to_string()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
        }
    }
}
