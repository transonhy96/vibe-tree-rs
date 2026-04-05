use crate::types::Theme;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSettings {
    pub font_family: String,
    pub font_size: f32,
    pub cursor_blink: bool,
    pub scrollback: u32,
    pub tab_stop_width: u8,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_family: "monospace".into(),
            font_size: 16.0,
            cursor_blink: true,
            scrollback: 10000,
            tab_stop_width: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceState {
    pub open_paths: Vec<PathBuf>,
    pub active_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub theme: Theme,
    pub terminal: TerminalSettings,
    pub recent_projects: Vec<PathBuf>,
    #[serde(default)]
    pub workspace_state: WorkspaceState,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            terminal: TerminalSettings::default(),
            recent_projects: Vec::new(),
            workspace_state: WorkspaceState::default(),
        }
    }
}

impl AppConfig {
    pub fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("vibetree"))
    }

    pub fn config_path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("config.toml"))
    }

    pub fn load() -> Self {
        Self::config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let dir = Self::config_dir().ok_or("no config dir")?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
