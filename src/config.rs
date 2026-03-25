use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level configuration, loaded from TOML.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// General settings.
    pub general: GeneralConfig,

    /// Commands for spawning external programs.
    pub commands: CommandsConfig,

    /// Workspace definitions.
    pub workspaces: Vec<WorkspaceConfig>,

    /// Window placement rules (like Notion's winprops).
    #[serde(default)]
    #[allow(dead_code)]
    pub winprops: Vec<WinProp>,

    /// Named keybinding profiles. "i3_neo" and "notion" are built-in defaults.
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    /// Which keybinding profile to use.
    pub active_profile: String,

    /// Appearance settings.
    pub appearance: AppearanceConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Use physical key positions (layout index 0) instead of logical keysyms.
    /// This makes keybindings work identically across keyboard layouts.
    pub physical_keys: bool,

    /// XKB layout index to use when physical_keys is true.
    /// 0 = first layout (usually the base layout like "de" or "us").
    pub physical_layout_index: u32,

    /// Focus follows mouse pointer into frames.
    pub focus_follows_mouse: bool,

    /// Warp cursor to focused frame on keyboard focus change.
    pub cursor_follows_focus: bool,

    /// Default split ratio for new splits.
    pub default_split_ratio: f32,

    /// Gap between frames in pixels.
    pub gap: u32,

    /// Border width in pixels.
    pub border_width: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CommandsConfig {
    pub terminal: String,
    pub launcher: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceConfig {
    pub name: String,
    /// Which output this workspace is assigned to (by name, e.g. "HDMI-0").
    pub output: Option<String>,
    /// Initial layout: "hsplit" or "vsplit". Default is a single frame.
    #[serde(default)]
    pub initial_layout: Option<String>,
}

/// A window placement rule, like Notion's winprops.
/// Routes windows matching a pattern to a specific named frame or workspace.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WinProp {
    /// Regex pattern matched against app_id.
    pub app_id: Option<String>,
    /// Regex pattern matched against window title.
    pub title: Option<String>,
    /// Target workspace name.
    pub workspace: Option<String>,
    /// Target named frame within the workspace.
    pub frame: Option<String>,
    /// Force floating.
    #[serde(default)]
    pub floating: bool,
}

/// A keybinding profile (a complete set of bindings).
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ProfileConfig {
    pub bindings: Vec<BindingConfig>,
    pub resize_bindings: Vec<BindingConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BindingConfig {
    /// Modifier keys: "super", "alt", "shift", "ctrl" (combined with +).
    pub modifiers: String,
    /// Key name (xkb keysym name, e.g. "space", "1", "Return").
    pub key: String,
    /// Action to perform.
    pub action: String,
    /// Arguments for the action.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    // ── Window border colors (sent to River) ──
    /// Active frame border color (hex).
    pub active_border: String,
    /// Inactive frame border color (hex).
    pub inactive_border: String,
    /// Empty frame border color (hex).
    pub empty_border: String,
    /// Urgent frame border color (hex).
    pub urgent_border: String,

    // ── Tab bar colors ──
    /// Active tab background in unfocused frame (hex).
    pub tab_active: String,
    /// Active tab background in focused frame (hex).
    pub tab_focused_active: String,
    /// Inactive tab background (hex).
    pub tab_inactive: String,
    /// Separator between tabs (hex).
    pub tab_separator: String,
    /// Active tab underline in focused frame (hex).
    pub tab_underline_focused: String,
    /// Active tab underline in unfocused frame (hex).
    pub tab_underline_unfocused: String,
    /// Focused active tab gradient start color (hex, left side). Empty to disable gradient.
    pub tab_gradient_start: String,
    /// Focused active tab gradient end color (hex, right side). Empty to disable gradient.
    pub tab_gradient_end: String,
    /// Unfocused active tab gradient start color (hex, left side). Empty to disable gradient.
    pub tab_active_gradient_start: String,
    /// Unfocused active tab gradient end color (hex, right side). Empty to disable gradient.
    pub tab_active_gradient_end: String,
    /// Inactive tab gradient start color (hex, left side). Empty to disable gradient.
    pub tab_inactive_gradient_start: String,
    /// Inactive tab gradient end color (hex, right side). Empty to disable gradient.
    pub tab_inactive_gradient_end: String,
    /// Active tab text color (hex).
    pub tab_text_active: String,
    /// Inactive tab text color (hex).
    pub tab_text_inactive: String,

    // ── Empty frame indicator colors ──
    /// Focused empty frame border (hex).
    pub empty_focused: String,
    /// Unfocused empty frame border (hex).
    pub empty_unfocused: String,

    // ── Resize highlight ──
    /// Color for the resize boundary highlight (hex with alpha, e.g. "#cba6f780").
    pub resize_highlight: String,

    // ── Waybar workspace colors (per-monitor) ──
    /// Colors for workspace indicators per monitor (hex, up to 4).
    pub monitor_colors: Vec<String>,
    /// Background color for focused workspace in waybar (hex).
    pub waybar_focused_bg: String,
}

/// Pre-parsed ARGB8888 colors for use in pixel rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Colors {
    pub tab_active: u32,
    pub tab_focused_active: u32,
    pub tab_inactive: u32,
    pub tab_separator: u32,
    pub tab_underline_focused: u32,
    pub tab_underline_unfocused: u32,
    pub tab_gradient_start: Option<u32>,
    pub tab_gradient_end: Option<u32>,
    pub tab_active_gradient_start: Option<u32>,
    pub tab_active_gradient_end: Option<u32>,
    pub tab_inactive_gradient_start: Option<u32>,
    pub tab_inactive_gradient_end: Option<u32>,
    pub tab_text_active: u32,
    pub tab_text_inactive: u32,
    pub empty_focused: u32,
    pub empty_unfocused: u32,
}

/// Parse a hex color string (#RRGGBB or #RRGGBBAA) to ARGB8888 u32.
pub fn hex_to_argb(hex: &str) -> u32 {
    let hex = hex.trim_start_matches('#');
    match hex.len() {
        6 => {
            let v = u32::from_str_radix(hex, 16).unwrap_or(0x333333);
            0xFF000000 | v
        }
        8 => {
            let v = u32::from_str_radix(hex, 16).unwrap_or(0xFF333333);
            // Input is RRGGBBAA, convert to AARRGGBB
            let r = (v >> 24) & 0xFF;
            let g = (v >> 16) & 0xFF;
            let b = (v >> 8) & 0xFF;
            let a = v & 0xFF;
            (a << 24) | (r << 16) | (g << 8) | b
        }
        _ => 0xFF333333,
    }
}

impl AppearanceConfig {
    /// Parse all hex color strings into ARGB8888 values.
    pub fn colors(&self) -> Colors {
        Colors {
            tab_active: hex_to_argb(&self.tab_active),
            tab_focused_active: hex_to_argb(&self.tab_focused_active),
            tab_inactive: hex_to_argb(&self.tab_inactive),
            tab_gradient_start: if self.tab_gradient_start.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_gradient_start))
            },
            tab_gradient_end: if self.tab_gradient_end.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_gradient_end))
            },
            tab_active_gradient_start: if self.tab_active_gradient_start.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_active_gradient_start))
            },
            tab_active_gradient_end: if self.tab_active_gradient_end.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_active_gradient_end))
            },
            tab_inactive_gradient_start: if self.tab_inactive_gradient_start.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_inactive_gradient_start))
            },
            tab_inactive_gradient_end: if self.tab_inactive_gradient_end.is_empty() {
                None
            } else {
                Some(hex_to_argb(&self.tab_inactive_gradient_end))
            },
            tab_separator: hex_to_argb(&self.tab_separator),
            tab_underline_focused: hex_to_argb(&self.tab_underline_focused),
            tab_underline_unfocused: hex_to_argb(&self.tab_underline_unfocused),
            tab_text_active: hex_to_argb(&self.tab_text_active),
            tab_text_inactive: hex_to_argb(&self.tab_text_inactive),
            empty_focused: hex_to_argb(&self.empty_focused),
            empty_unfocused: hex_to_argb(&self.empty_unfocused),
        }
    }
}

// ── Defaults ──────────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            commands: CommandsConfig::default(),
            workspaces: default_workspaces(),
            winprops: Vec::new(),
            profiles: HashMap::new(),
            active_profile: "i3_neo".to_string(),
            appearance: AppearanceConfig::default(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            physical_keys: true,
            physical_layout_index: 0,
            focus_follows_mouse: true,
            cursor_follows_focus: true,
            default_split_ratio: 0.5,
            gap: 4,
            border_width: 2,
        }
    }
}

impl Default for CommandsConfig {
    fn default() -> Self {
        Self {
            terminal: "contour".to_string(),
            launcher: vec!["rofi".to_string(), "-show".to_string(), "combi".to_string()],
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            active_border: "#9b8ec4".to_string(),
            inactive_border: "#2a2636".to_string(),
            empty_border: "#5e5775".to_string(),
            urgent_border: "#c45b84".to_string(),

            tab_active: "#7c6f9b".to_string(),
            tab_focused_active: "#9b8ec4".to_string(),
            tab_inactive: "#1e1b26".to_string(),
            tab_gradient_start: "#5a2ab5".to_string(),
            tab_gradient_end: "#a8407f".to_string(),
            tab_active_gradient_start: "#3a2850".to_string(),
            tab_active_gradient_end: "#201828".to_string(),
            tab_inactive_gradient_start: "#3a2850".to_string(),
            tab_inactive_gradient_end: "#201828".to_string(),
            tab_separator: "#5e5775".to_string(),
            tab_underline_focused: "#cdb4ff".to_string(),
            tab_underline_unfocused: "#7c6f9b".to_string(),
            tab_text_active: "#ede8f5".to_string(),
            tab_text_inactive: "#8a8399".to_string(),

            empty_focused: "#7c6f9b".to_string(),
            empty_unfocused: "#332e42".to_string(),

            resize_highlight: "#cba6f780".to_string(),

            monitor_colors: vec![
                "#b4a0e5".to_string(),
                "#a6c9a1".to_string(),
                "#e5cfa6".to_string(),
                "#d68ba8".to_string(),
            ],
            waybar_focused_bg: "#2a2636".to_string(),
        }
    }
}

fn default_workspaces() -> Vec<WorkspaceConfig> {
    vec![
        WorkspaceConfig {
            name: "main".to_string(),
            output: None,
            initial_layout: Some("hsplit".to_string()),
        },
        WorkspaceConfig {
            name: "secondary".to_string(),
            output: None,
            initial_layout: None,
        },
        WorkspaceConfig {
            name: "utility".to_string(),
            output: None,
            initial_layout: None,
        },
        WorkspaceConfig {
            name: "social".to_string(),
            output: None,
            initial_layout: None,
        },
        WorkspaceConfig {
            name: "work".to_string(),
            output: None,
            initial_layout: None,
        },
        WorkspaceConfig {
            name: "term".to_string(),
            output: None,
            initial_layout: None,
        },
    ]
}

// ── Loading ──────────────────────────────────────────────────────────────

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(config) => {
                        log::info!("Loaded config from {}", path.display());
                        return config;
                    }
                    Err(e) => {
                        log::error!("Failed to parse config {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    log::error!("Failed to read config {}: {e}", path.display());
                }
            }
        } else {
            log::info!("No config at {}, using defaults", path.display());
        }
        Config::default()
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("notion-river")
        .join("config.toml")
}
