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
    /// Active frame border color (hex).
    pub active_border: String,
    /// Inactive frame border color (hex).
    pub inactive_border: String,
    /// Empty frame border color (hex).
    pub empty_border: String,
    /// Urgent frame border color (hex).
    pub urgent_border: String,
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
            active_border: "#4c7899".to_string(),
            inactive_border: "#333333".to_string(),
            empty_border: "#555555".to_string(),
            urgent_border: "#900000".to_string(),
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
