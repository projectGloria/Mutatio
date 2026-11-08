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

impl TaskbarState {
    /// Parses the CLI / IPC preset names. Single source of truth so the CLI,
    /// the `--daemon-start` path and the IPC handler can never drift apart.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "normal" | "default" => Some(Self::Normal),
            "clear" => Some(Self::Clear),
            "opaque" => Some(Self::Opaque),
            "blur" => Some(Self::Blur),
            "acrylic" => Some(Self::Acrylic),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Clear => "clear",
            Self::Opaque => "opaque",
            Self::Blur => "blur",
            Self::Acrylic => "acrylic",
        }
    }
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

    /// Converts to Windows COLORREF/ABGR format (0xAABBGGRR)
    pub const fn to_abgr(self) -> u32 {
        ((self.a as u32) << 24) | ((self.b as u32) << 16) | ((self.g as u32) << 8) | (self.r as u32)
    }

    /// Inverse of `to_abgr` — used to unpack colors smuggled through a window message LPARAM.
    pub const fn from_abgr(v: u32) -> Self {
        Self::new(v as u8, (v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8)
    }

    /// Parses `#RRGGBB` or `#RRGGBBAA`.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let clean = hex.trim().trim_start_matches('#');
        if !clean.is_ascii() {
            return None; // byte slicing below would panic on a multi-byte char boundary
        }
        let byte = |i: usize| u8::from_str_radix(clean.get(i..i + 2)?, 16).ok();
        match clean.len() {
            6 => Some(Self::new(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some(Self::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// Linear interpolation between two colors
    pub fn lerp(self, other: Self, factor: f32) -> Self {
        let f = factor.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::transparent()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateAppearance {
    pub state: TaskbarState,
    pub color: Color,
    pub show_line: bool,
}

impl StateAppearance {
    /// Built-in look for each preset. Previously copy-pasted in three places in main.rs.
    pub const fn preset(state: TaskbarState) -> Self {
        let (color, show_line) = match state {
            TaskbarState::Normal => (Color::transparent(), true),
            TaskbarState::Clear => (Color::transparent(), false),
            TaskbarState::Blur => (Color::new(20, 20, 20, 120), false),
            TaskbarState::Acrylic => (Color::new(30, 30, 30, 180), false),
            TaskbarState::Opaque => (Color::new(0, 0, 0, 255), false),
        };
        Self { state, color, show_line }
    }
}

impl Default for StateAppearance {
    fn default() -> Self {
        Self::preset(TaskbarState::Clear)
    }
}

// `#[serde(default)]` = missing keys fall back to `Config::default()` per field, so adding a
// field in a future version no longer invalidates every existing config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Appearance when viewing the clean desktop
    pub desktop: StateAppearance,

    /// Appearance when regular windows are visible on the taskbar's monitor
    pub visible_window: StateAppearance,

    /// Appearance when a window is maximized on the taskbar's monitor
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

    /// Restore the stock taskbar while a fullscreen app/game owns the monitor
    pub game_mode_pause: bool,

    /// Reserved; not implemented yet.
    pub battery_saver_fallback: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            desktop: StateAppearance::preset(TaskbarState::Clear),
            visible_window: StateAppearance::preset(TaskbarState::Blur),
            maximized_window: StateAppearance::preset(TaskbarState::Opaque),
            start_opened: StateAppearance::preset(TaskbarState::Acrylic),
            search_opened: StateAppearance::preset(TaskbarState::Acrylic),
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
        match fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<Config>(&content) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    // Old behaviour silently overwrote the user's file with defaults on any typo.
                    // Keep their work in a .bak so a hand-edit mistake is recoverable.
                    let backup = path.with_extension("json.bak");
                    let _ = fs::copy(path, &backup);
                    eprintln!("[!] Invalid config ({e}); backed up to {backup:?}, using defaults");
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                // Unreadable (locked, permissions) — don't clobber it, just run with defaults.
                eprintln!("[!] Could not read config ({e}); using defaults");
                return Config::default();
            }
        }
        let cfg = Config::default();
        let _ = cfg.save(path);
        cfg
    }

    /// Write-to-temp + rename so a crash or power loss mid-write can't leave a truncated file
    /// (which `load_or_create` would then treat as corrupt).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, content)?;
        fs::rename(&tmp, path) // MoveFileExW(MOVEFILE_REPLACE_EXISTING) on Windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(Color::from_hex("#FF000080"), Some(Color::new(255, 0, 0, 128)));
        assert_eq!(Color::from_hex("00ff00"), Some(Color::new(0, 255, 0, 255)));
        assert_eq!(Color::from_hex("#12345"), None);
        assert_eq!(Color::from_hex("#GG0000"), None);
        // Multi-byte input used to be sliced by byte index and could panic.
        assert_eq!(Color::from_hex("#ééé"), None);
    }

    #[test]
    fn abgr_round_trip() {
        let c = Color::new(1, 2, 3, 4);
        assert_eq!(c.to_abgr(), 0x0403_0201);
        assert_eq!(Color::from_abgr(c.to_abgr()), c);
    }

    #[test]
    fn lerp_endpoints() {
        let a = Color::new(0, 0, 0, 0);
        let b = Color::new(200, 100, 50, 255);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 2.0), b); // clamped
    }

    #[test]
    fn state_names_round_trip() {
        for s in [TaskbarState::Normal, TaskbarState::Clear, TaskbarState::Opaque, TaskbarState::Blur, TaskbarState::Acrylic] {
            assert_eq!(TaskbarState::parse(s.name()), Some(s));
        }
        assert_eq!(TaskbarState::parse("  BLUR "), Some(TaskbarState::Blur));
        assert_eq!(TaskbarState::parse("nope"), None);
    }

    #[test]
    fn partial_config_uses_defaults() {
        // A config written by an older/newer version with missing keys must still load.
        let cfg: Config = serde_json::from_str(r#"{ "smooth_transitions": false, "desktop": { "state": "opaque" } }"#).unwrap();
        assert!(!cfg.smooth_transitions);
        assert_eq!(cfg.desktop.state, TaskbarState::Opaque);
        assert_eq!(cfg.transition_duration_ms, Config::default().transition_duration_ms);
        assert_eq!(cfg.maximized_window, Config::default().maximized_window);
    }

    #[test]
    fn invalid_config_is_backed_up_not_lost() {
        let dir = std::env::temp_dir().join(format!("mutatio-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("config.json");
        fs::write(&path, "{ not json").unwrap();

        let cfg = Config::load_or_create(&path);
        assert!(cfg.dynamic_modes_enabled);
        assert_eq!(fs::read_to_string(path.with_extension("json.bak")).unwrap(), "{ not json");

        let _ = fs::remove_dir_all(&dir);
    }
}
