use crate::layout::Direction;

/// All possible actions the WM can perform.
/// These are triggered by keybindings and mapped from config strings.
#[derive(Debug, Clone)]
pub enum Action {
    // ── Window management ──
    /// Close the focused window. If frame is empty, close (unsplit) the frame.
    Close,
    /// Toggle fullscreen for the focused window.
    ToggleFullscreen,
    /// Toggle floating for the focused window.
    ToggleFloat,
    /// Toggle the split orientation (H↔V) of the parent of the focused frame.
    ToggleSplit,

    // ── Focus ──
    /// Move focus to the frame in the given direction.
    FocusDirection(Direction),
    /// Focus the next tab in the current frame.
    FocusNextTab,
    /// Focus the previous tab in the current frame.
    FocusPrevTab,
    /// Focus parent (reserved for future nested container support).
    FocusParent,

    // ── Moving windows ──
    /// Move the focused window to the frame in the given direction.
    MoveDirection(Direction),
    /// Move the focused window to a named workspace.
    MoveToWorkspace(String),

    // ── Layout manipulation ──
    /// Split the focused frame horizontally (side by side).
    SplitHorizontal,
    /// Split the focused frame vertically (top/bottom).
    SplitVertical,
    /// Remove the focused frame (unsplit), only if empty.
    Unsplit,

    // ── Workspaces ──
    /// Switch to workspace by name.
    SwitchWorkspace(String),

    // ── Resize mode ──
    /// Enter resize mode.
    EnterResizeMode,
    /// Exit resize mode (back to normal).
    ExitResizeMode,
    /// Resize the focused frame in the given direction (while in resize mode).
    Resize(Direction),

    // ── Spawning ──
    /// Spawn the configured terminal.
    SpawnTerminal,
    /// Spawn the configured launcher.
    SpawnLauncher,
    /// Spawn an arbitrary command.
    Spawn(Vec<String>),

    // ── Session ──
    /// Exit the window manager.
    Exit,
    /// Reload configuration.
    ReloadConfig,

    /// No-op (used as placeholder).
    None,
}

impl Action {
    /// Parse an action from a config string + arguments.
    pub fn from_config(action: &str, args: &[String]) -> Self {
        match action {
            "close" => Action::Close,
            "toggle_fullscreen" | "fullscreen" => Action::ToggleFullscreen,
            "toggle_float" | "floating" => Action::ToggleFloat,
            "toggle_split" => Action::ToggleSplit,

            "focus" => {
                if let Some(dir) = args.first().and_then(|s| Direction::from_str(s)) {
                    Action::FocusDirection(dir)
                } else {
                    log::warn!("focus action needs direction arg (left/right/up/down)");
                    Action::None
                }
            }
            "focus_next_tab" | "next_tab" => Action::FocusNextTab,
            "focus_prev_tab" | "prev_tab" => Action::FocusPrevTab,
            "focus_parent" => Action::FocusParent,

            "move" => {
                if let Some(dir) = args.first().and_then(|s| Direction::from_str(s)) {
                    Action::MoveDirection(dir)
                } else {
                    log::warn!("move action needs direction arg (left/right/up/down)");
                    Action::None
                }
            }
            "move_to_workspace" => {
                if let Some(name) = args.first() {
                    Action::MoveToWorkspace(name.clone())
                } else {
                    log::warn!("move_to_workspace needs workspace name arg");
                    Action::None
                }
            }

            "split_horizontal" | "split_h" | "hsplit" => Action::SplitHorizontal,
            "split_vertical" | "split_v" | "vsplit" => Action::SplitVertical,
            "unsplit" | "remove_frame" => Action::Unsplit,

            "workspace" => {
                if let Some(name) = args.first() {
                    Action::SwitchWorkspace(name.clone())
                } else {
                    log::warn!("workspace action needs workspace name arg");
                    Action::None
                }
            }

            "resize_mode" => Action::EnterResizeMode,
            "exit_resize_mode" | "normal_mode" => Action::ExitResizeMode,
            "resize" => {
                if let Some(dir) = args.first().and_then(|s| Direction::from_str(s)) {
                    Action::Resize(dir)
                } else {
                    log::warn!("resize action needs direction arg");
                    Action::None
                }
            }

            "spawn_terminal" | "terminal" => Action::SpawnTerminal,
            "spawn_launcher" | "launcher" => Action::SpawnLauncher,
            "spawn" => {
                if args.is_empty() {
                    log::warn!("spawn action needs at least one arg");
                    Action::None
                } else {
                    Action::Spawn(args.to_vec())
                }
            }

            "exit" | "exit_session" => Action::Exit,
            "reload" | "reload_config" => Action::ReloadConfig,

            other => {
                log::warn!("Unknown action: {other}");
                Action::None
            }
        }
    }
}
