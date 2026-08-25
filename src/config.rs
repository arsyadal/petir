//! TOML config, loaded from `%APPDATA%/rterm/rterm.toml` on Windows
//! (via `dirs::config_dir()`). Falls back to sane defaults if missing —
//! rterm should run with zero config, same philosophy as Alacritty.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    /// Enable programming ligatures (=>, ->, ==, etc.) via font shaping.
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Cascadia Code".into(),
            size: 14.0,
            ligatures: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct WindowConfig {
    pub padding_x: u32,
    pub padding_y: u32,
    pub opacity: f32,
    pub decorations: bool,
    /// Cap FPS to save battery/CPU on laptops. 0 = uncapped (present as fast as GPU allows).
    pub max_fps: u32,
    /// Wait for vertical blank before presenting. Off trades visible tearing
    /// for a little less input-to-photon latency.
    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding_x: 6,
            padding_y: 6,
            opacity: 1.0,
            decorations: true,
            max_fps: 144,
            vsync: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct ClipboardConfig {
    /// THE fix vs Alacritty: plain Ctrl+C copies when there's a selection
    /// and falls through to SIGINT otherwise; plain Ctrl+V pastes. No
    /// Ctrl+Shift+C/V required. Set false to restore Alacritty-style
    /// Ctrl+Shift+C/V-only behavior.
    pub smart_ctrl_c_ctrl_v: bool,
    /// Also copy selection to clipboard automatically on mouse-up (X11-style).
    pub copy_on_select: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            smart_ctrl_c_ctrl_v: true,
            copy_on_select: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct ScrollConfig {
    pub history_lines: usize,
    pub lines_per_tick: usize,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            history_lines: 100_000,
            lines_per_tick: 3,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Colors {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: "#1e1e1e".into(),
            foreground: "#d8d8d8".into(),
            cursor: "#ffffff".into(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub clipboard: ClipboardConfig,
    pub scroll: ScrollConfig,
    pub colors: Colors,
    /// Shell to launch. Empty = auto-detect (pwsh > powershell > cmd on Windows).
    pub shell: String,
}

impl Config {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("petir").join("petir.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("Gagal parse {path:?}: {e}, pakai default config");
                Self::default()
            }),
            Err(_) => {
                // First run: write out defaults so the user has something to edit.
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, DEFAULT_TOML);
                Self::default()
            }
        }
    }
}

const DEFAULT_TOML: &str = r##"# Petir config
# File ini dibaca ulang otomatis saat disimpan (hot-reload).

[font]
family = "Cascadia Code"
size = 14.0
ligatures = true

[window]
padding_x = 6
padding_y = 6
opacity = 1.0
decorations = true
max_fps = 144 # 0 = uncapped
# vsync = true  # false: latency terendah, tapi bisa tearing

[clipboard]
# Ctrl+C copy kalau ada seleksi, Ctrl+V paste langsung -- tanpa Ctrl+Shift.
smart_ctrl_c_ctrl_v = true
copy_on_select = true

[scroll]
history_lines = 100000
lines_per_tick = 3

[colors]
background = "#1e1e1e"
foreground = "#d8d8d8"
cursor = "#ffffff"

# shell = "pwsh.exe" # kosongkan untuk auto-detect
"##;
