use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskbarState {
    Normal,
    Clear,
    Opaque,
    Blur,
    Acrylic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    #[allow(dead_code)]
    pub const fn black_semi(alpha: u8) -> Self {
        Self::new(16, 16, 16, alpha)
    }

    /// Converts to Windows COLORREF/ABGR format (0xAABBGGRR)
    pub fn to_abgr(self) -> u32 {
        ((self.a as u32) << 24)
            | ((self.b as u32) << 16)
            | ((self.g as u32) << 8)
            | (self.r as u32)
    }

    /// Converts to ARGB format (0xAARRGGBB)
    #[allow(dead_code)]
    pub fn to_argb(self) -> u32 {
        ((self.a as u32) << 24)
            | ((self.r as u32) << 16)
            | ((self.g as u32) << 8)
            | (self.b as u32)
    }

    /// Linear interpolation between two colors
    pub fn lerp(self, other: Self, factor: f32) -> Self {
        let f = factor.clamp(0.0, 1.0);
        let inv = 1.0 - f;
        Self {
            r: ((self.r as f32 * inv) + (other.r as f32 * f)).round() as u8,
            g: ((self.g as f32 * inv) + (other.g as f32 * f)).round() as u8,
            b: ((self.b as f32 * inv) + (other.b as f32 * f)).round() as u8,
            a: ((self.a as f32 * inv) + (other.a as f32 * f)).round() as u8,
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::transparent()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateAppearance {
    pub state: TaskbarState,
    pub color: Color,
    pub show_line: bool,
}

impl Default for StateAppearance {
    fn default() -> Self {
        Self {
            state: TaskbarState::Clear,
            color: Color::transparent(),
            show_line: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Appearance when viewing the clean desktop
    pub desktop: StateAppearance,

    /// Appearance when regular windows are visible
    pub visible_window: StateAppearance,

    /// Appearance when a window is maximized
    pub maximized_window: StateAppearance,

    /// Appearance when Start Menu is open
    pub start_opened: StateAppearance,

    /// Appearance when Search flyout is open
    pub search_opened: StateAppearance,

    /// Whether to enable dynamic mode transitions
    pub dynamic_modes_enabled: bool,

    /// Enable smooth animated fade transitions between states
    pub smooth_transitions: bool,

    /// Transition duration in milliseconds (e.g. 150ms)
    pub transition_duration_ms: u64,

    /// Auto-disarm/sleep during fullscreen games (0% CPU, no anti-cheat hooks)
    pub game_mode_pause: bool,

    /// Battery saver fallback
    pub battery_saver_fallback: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            desktop: StateAppearance {
                state: TaskbarState::Clear,
                color: Color::transparent(),
                show_line: false,
            },
            visible_window: StateAppearance {
                state: TaskbarState::Blur,
                color: Color::new(20, 20, 20, 120),
                show_line: false,
            },
            maximized_window: StateAppearance {
                state: TaskbarState::Opaque,
                color: Color::new(0, 0, 0, 255),
                show_line: false,
            },
            start_opened: StateAppearance {
                state: TaskbarState::Acrylic,
                color: Color::new(30, 30, 30, 180),
                show_line: false,
            },
            search_opened: StateAppearance {
                state: TaskbarState::Acrylic,
                color: Color::new(30, 30, 30, 180),
                show_line: false,
            },
            dynamic_modes_enabled: true,
            smooth_transitions: true,
            transition_duration_ms: 180,
            game_mode_pause: true,
            battery_saver_fallback: true,
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let mut dir = PathBuf::from(app_data);
            dir.push("Mutatio");
            let _ = fs::create_dir_all(&dir);
            dir.push("config.json");
            return dir;
        }
        PathBuf::from("mutatio.json")
    }

    pub fn load_or_create(path: &Path) -> Self {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(cfg) = serde_json::from_str::<Config>(&content) {
                    return cfg;
                }
            }
        }
        let cfg = Config::default();
        let _ = cfg.save(path);
        cfg
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, content)
    }
}
