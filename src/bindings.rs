use crate::actions::Action;
use crate::config::{BindingConfig, Config};

/// A parsed keybinding ready to be registered with River.
#[derive(Debug)]
pub struct Binding {
    pub keysym: u32,
    pub modifiers: u32,
    pub action: Action,
    /// If true, use layout override for physical key behavior.
    pub layout_override: Option<u32>,
}

/// Modifier flag constants matching River's river_seat_v1.modifiers enum.
/// These are bitmask values.
pub mod modifiers {
    pub const SHIFT: u32 = 1;
    #[allow(dead_code)]
    pub const CAPS_LOCK: u32 = 2;
    pub const CTRL: u32 = 4;
    pub const MOD1: u32 = 8; // Alt
    pub const MOD2: u32 = 16; // Num Lock
    pub const MOD3: u32 = 32;
    pub const MOD4: u32 = 64; // Super/Logo
    pub const MOD5: u32 = 128;
}

/// Parse modifier string like "super+shift" into bitmask.
pub fn parse_modifiers(s: &str) -> u32 {
    let mut mods = 0u32;
    for part in s.split('+') {
        match part.trim().to_lowercase().as_str() {
            "super" | "mod4" | "logo" => mods |= modifiers::MOD4,
            "alt" | "mod1" => mods |= modifiers::MOD1,
            "shift" => mods |= modifiers::SHIFT,
            "ctrl" | "control" => mods |= modifiers::CTRL,
            "mod2" => mods |= modifiers::MOD2,
            "mod3" => mods |= modifiers::MOD3,
            "mod5" => mods |= modifiers::MOD5,
            "" => {}
            other => log::warn!("Unknown modifier: {other}"),
        }
    }
    mods
}

/// Look up an xkb keysym by name.
/// Falls back to case-insensitive search if exact match fails.
pub fn keysym_from_name(name: &str) -> Option<u32> {
    use xkbcommon::xkb::keysyms::KEY_NoSymbol;
    // Use xkbcommon to resolve keysym names
    let sym = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_NO_FLAGS);
    if sym.raw() == KEY_NoSymbol {
        // Try case-insensitive
        let sym = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_CASE_INSENSITIVE);
        if sym.raw() == KEY_NoSymbol {
            None
        } else {
            Some(sym.into())
        }
    } else {
        Some(sym.into())
    }
}

/// Parse a single binding config entry into a Binding.
pub fn parse_binding(
    cfg: &BindingConfig,
    physical_keys: bool,
    layout_index: u32,
) -> Option<Binding> {
    let keysym = match keysym_from_name(&cfg.key) {
        Some(sym) => sym,
        None => {
            log::warn!("Unknown key name: '{}'", cfg.key);
            return None;
        }
    };

    let mods = parse_modifiers(&cfg.modifiers);
    let action = Action::from_config(&cfg.action, &cfg.args);

    Some(Binding {
        keysym,
        modifiers: mods,
        action,
        layout_override: if physical_keys {
            Some(layout_index)
        } else {
            None
        },
    })
}

/// Parse all bindings from config for a given mode (normal or resize).
pub fn parse_all_bindings(
    bindings: &[BindingConfig],
    physical_keys: bool,
    layout_index: u32,
) -> Vec<Binding> {
    bindings
        .iter()
        .filter_map(|cfg| parse_binding(cfg, physical_keys, layout_index))
        .collect()
}

// ── Built-in binding profiles ────────────────────────────────────────────

/// Generate the "i3_neo" keybinding profile matching the user's current i3 config.
/// Directions use Neo layout: i=left, a=down, l=up, e=right.
pub fn builtin_i3_neo_bindings() -> Vec<BindingConfig> {
    let mut b = Vec::new();

    // ── Basics ──
    bind(&mut b, "super", "space", "spawn_terminal", &[]);
    bind(&mut b, "super", "c", "close", &[]);
    bind(&mut b, "super", "o", "spawn_launcher", &[]);
    bind(&mut b, "super", "Return", "toggle_fullscreen", &[]);

    // ── Focus (Neo: i/a/l/e) ──
    bind(&mut b, "super", "i", "focus", &["left"]);
    bind(&mut b, "super", "a", "focus", &["down"]);
    bind(&mut b, "super", "l", "focus", &["up"]);
    bind(&mut b, "super", "e", "focus", &["right"]);
    // Arrow key alternatives
    bind(&mut b, "super", "Left", "focus", &["left"]);
    bind(&mut b, "super", "Down", "focus", &["down"]);
    bind(&mut b, "super", "Up", "focus", &["up"]);
    bind(&mut b, "super", "Right", "focus", &["right"]);

    // ── Move window (Neo: i/a/l/e + Shift) ──
    bind(&mut b, "super+shift", "i", "move", &["left"]);
    bind(&mut b, "super+shift", "a", "move", &["down"]);
    bind(&mut b, "super+shift", "l", "move", &["up"]);
    bind(&mut b, "super+shift", "e", "move", &["right"]);
    bind(&mut b, "super+shift", "Left", "move", &["left"]);
    bind(&mut b, "super+shift", "Down", "move", &["down"]);
    bind(&mut b, "super+shift", "Up", "move", &["up"]);
    bind(&mut b, "super+shift", "Right", "move", &["right"]);

    // ── Layout manipulation ──
    bind(&mut b, "super", "b", "split_horizontal", &[]);
    bind(&mut b, "super", "v", "split_vertical", &[]);
    bind(&mut b, "super", "t", "toggle_split", &[]);

    // ── Tabbing ──
    bind(&mut b, "super", "w", "next_tab", &[]);
    bind(&mut b, "super", "q", "prev_tab", &[]);
    bind(&mut b, "super", "n", "next_tab", &[]);
    bind(&mut b, "super", "p", "prev_tab", &[]);

    // ── Workspaces (Super+1..4 on primary, Alt+1..3 on secondary) ──
    bind(&mut b, "super", "1", "workspace", &["main"]);
    bind(&mut b, "super", "2", "workspace", &["secondary"]);
    bind(&mut b, "super", "3", "workspace", &["utility"]);
    bind(&mut b, "super", "4", "workspace", &["steam-games"]);
    bind(&mut b, "alt", "1", "workspace", &["social"]);
    bind(&mut b, "alt", "2", "workspace", &["work"]);
    bind(&mut b, "alt", "3", "workspace", &["term"]);

    // ── Move to workspace ──
    bind(&mut b, "super+shift", "1", "move_to_workspace", &["main"]);
    bind(
        &mut b,
        "super+shift",
        "2",
        "move_to_workspace",
        &["secondary"],
    );
    bind(
        &mut b,
        "super+shift",
        "3",
        "move_to_workspace",
        &["utility"],
    );
    bind(
        &mut b,
        "super+shift",
        "4",
        "move_to_workspace",
        &["steam-games"],
    );
    bind(&mut b, "alt+shift", "1", "move_to_workspace", &["social"]);
    bind(&mut b, "alt+shift", "2", "move_to_workspace", &["work"]);
    bind(&mut b, "alt+shift", "3", "move_to_workspace", &["term"]);

    // ── Floating ──
    bind(&mut b, "super+shift", "space", "toggle_float", &[]);

    // ── Resize mode ──
    bind(&mut b, "super", "r", "resize_mode", &[]);

    // ── Session ──
    bind(&mut b, "super+shift", "c", "reload_config", &[]);

    b
}

/// Resize mode bindings for the i3_neo profile.
pub fn builtin_i3_neo_resize_bindings() -> Vec<BindingConfig> {
    let mut b = Vec::new();

    // Neo directions
    bind(&mut b, "super", "i", "resize", &["left"]);
    bind(&mut b, "super", "a", "resize", &["down"]);
    bind(&mut b, "super", "l", "resize", &["up"]);
    bind(&mut b, "super", "e", "resize", &["right"]);
    // Arrow keys
    bind(&mut b, "", "Left", "resize", &["left"]);
    bind(&mut b, "", "Down", "resize", &["down"]);
    bind(&mut b, "", "Up", "resize", &["up"]);
    bind(&mut b, "", "Right", "resize", &["right"]);
    // Exit resize mode
    bind(&mut b, "", "space", "exit_resize_mode", &[]);
    bind(&mut b, "super", "space", "exit_resize_mode", &[]);
    bind(&mut b, "", "Escape", "exit_resize_mode", &[]);

    b
}

/// Generate the "notion" keybinding profile — my suggested alternative.
///
/// Design principles:
/// - Super is the primary modifier (same as i3_neo).
/// - Directions use standard HJKL (Vim) since physical_keys mode makes
///   this work regardless of active keyboard layout.
/// - Notion-style chord prefix: Super+K enters a "frame command" submap
///   for less-frequent operations (split, unsplit, tab switching).
/// - Workspace switching uses Super+1..9 (unified, no Alt split).
/// - Emphasis on the static tiling model: split/unsplit are prominent,
///   layout algorithms are absent.
pub fn builtin_notion_bindings() -> Vec<BindingConfig> {
    let mut b = Vec::new();

    // ── Basics ──
    bind(&mut b, "super", "Return", "spawn_terminal", &[]);
    bind(&mut b, "super", "c", "close", &[]);
    bind(&mut b, "super", "p", "spawn_launcher", &[]);
    bind(&mut b, "super", "f", "toggle_fullscreen", &[]);

    // ── Focus (Vim: h/j/k/l) ──
    bind(&mut b, "super", "h", "focus", &["left"]);
    bind(&mut b, "super", "j", "focus", &["down"]);
    bind(&mut b, "super", "k", "focus", &["up"]);
    bind(&mut b, "super", "l", "focus", &["right"]);

    // ── Move window ──
    bind(&mut b, "super+shift", "h", "move", &["left"]);
    bind(&mut b, "super+shift", "j", "move", &["down"]);
    bind(&mut b, "super+shift", "k", "move", &["up"]);
    bind(&mut b, "super+shift", "l", "move", &["right"]);

    // ── Layout manipulation ──
    // Super+s = split horizontal (mnemonic: "side-by-side")
    bind(&mut b, "super", "s", "split_horizontal", &[]);
    // Super+v = split vertical
    bind(&mut b, "super", "v", "split_vertical", &[]);
    // Super+t = toggle split orientation
    bind(&mut b, "super", "t", "toggle_split", &[]);
    // Super+x = unsplit / remove empty frame (mnemonic: "x out")
    bind(&mut b, "super", "x", "unsplit", &[]);

    // ── Tabbing ──
    // Super+Tab / Super+Shift+Tab for tab cycling (very natural)
    bind(&mut b, "super", "Tab", "next_tab", &[]);
    bind(&mut b, "super+shift", "Tab", "prev_tab", &[]);

    // ── Workspaces ──
    bind(&mut b, "super", "1", "workspace", &["main"]);
    bind(&mut b, "super", "2", "workspace", &["secondary"]);
    bind(&mut b, "super", "3", "workspace", &["utility"]);
    bind(&mut b, "super", "4", "workspace", &["social"]);
    bind(&mut b, "super", "5", "workspace", &["work"]);
    bind(&mut b, "super", "6", "workspace", &["term"]);

    // ── Move to workspace ──
    bind(&mut b, "super+shift", "1", "move_to_workspace", &["main"]);
    bind(
        &mut b,
        "super+shift",
        "2",
        "move_to_workspace",
        &["secondary"],
    );
    bind(
        &mut b,
        "super+shift",
        "3",
        "move_to_workspace",
        &["utility"],
    );
    bind(&mut b, "super+shift", "4", "move_to_workspace", &["social"]);
    bind(&mut b, "super+shift", "5", "move_to_workspace", &["work"]);
    bind(&mut b, "super+shift", "6", "move_to_workspace", &["term"]);

    // ── Floating ──
    bind(&mut b, "super+shift", "f", "toggle_float", &[]);

    // ── Resize mode ──
    bind(&mut b, "super", "r", "resize_mode", &[]);

    // ── Session ──
    bind(&mut b, "super+shift", "r", "reload_config", &[]);
    bind(&mut b, "super+shift", "e", "exit", &[]);

    b
}

/// Resize mode bindings for the notion profile.
pub fn builtin_notion_resize_bindings() -> Vec<BindingConfig> {
    let mut b = Vec::new();

    bind(&mut b, "", "h", "resize", &["left"]);
    bind(&mut b, "", "j", "resize", &["down"]);
    bind(&mut b, "", "k", "resize", &["up"]);
    bind(&mut b, "", "l", "resize", &["right"]);
    bind(&mut b, "", "Left", "resize", &["left"]);
    bind(&mut b, "", "Down", "resize", &["down"]);
    bind(&mut b, "", "Up", "resize", &["up"]);
    bind(&mut b, "", "Right", "resize", &["right"]);
    bind(&mut b, "", "Escape", "exit_resize_mode", &[]);
    bind(&mut b, "", "Return", "exit_resize_mode", &[]);

    b
}

/// Helper to build a BindingConfig.
fn bind(out: &mut Vec<BindingConfig>, mods: &str, key: &str, action: &str, args: &[&str]) {
    out.push(BindingConfig {
        modifiers: mods.to_string(),
        key: key.to_string(),
        action: action.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    });
}

/// Get bindings for a profile, falling back to built-in defaults.
pub fn get_profile_bindings(config: &Config) -> (Vec<BindingConfig>, Vec<BindingConfig>) {
    // Check if the active profile has custom bindings in config
    if let Some(profile) = config.profiles.get(&config.active_profile) {
        if !profile.bindings.is_empty() {
            return (profile.bindings.clone(), profile.resize_bindings.clone());
        }
    }

    // Fall back to built-in profiles
    match config.active_profile.as_str() {
        "notion" => (builtin_notion_bindings(), builtin_notion_resize_bindings()),
        "i3_neo" | _ => (builtin_i3_neo_bindings(), builtin_i3_neo_resize_bindings()),
    }
}
