# notion-river

A **static tiling** window manager for the [River](https://codeberg.org/river/river) Wayland compositor (0.4.x+), inspired by [Notion](https://notionwm.net/) (formerly Ion3).

## What is "static tiling"?

Unlike dynamic tiling WMs (i3, Sway, Hyprland) where the layout reflows every time a window opens or closes, notion-river uses **persistent frames**:

- The screen is divided into a tree of frames (cells) that you create manually with split commands
- Frames persist even when empty — they're the skeleton of your workspace
- Windows are placed *into* frames as **tabs** — multiple windows share a frame, one visible at a time
- Opening/closing a window never changes the layout — only explicit user actions (split, unsplit) do

This gives you a predictable, stable workspace layout that doesn't rearrange itself.

## Features

- **Static tiling** with manual split/unsplit (horizontal, vertical, toggle)
- **Tabbed frames** — multiple windows per frame, click tab bar to switch
- **Empty frame indicators** — visible wireframe for empty cells
- **Focus-follows-mouse** across frames and monitors (including empty frames)
- **Cursor-follows-focus** on keyboard navigation
- **Cross-monitor focus and window moving** with edge-position matching
- **Pointer drag** — left-drag moves windows between frames, right-drag resizes splits
- **Multi-monitor** with per-output workspace assignment
- **Layer-shell support** — waybar, notifications, rofi overlays
- **Waybar integration** — workspace indicators with per-monitor grouping
- **Media keys** — volume, brightness, playback controls
- **Physical key bindings** — work across keyboard layouts (Neo, Dvorak, etc.)
- **Two built-in keybinding profiles** — `i3_neo` (Neo layout) and `notion` (Vim-style)
- **State persistence** — layout and window placement saved/restored across WM restarts
- **Split moves active window** — splitting a multi-tab frame moves the current window to the new frame
- **TOML configuration**

## Requirements

- [River](https://codeberg.org/river/river) 0.4.x+ (built from source, uses `river-window-management-v1` protocol)
- Rust 1.75+
- `kanshi` or `wlr-randr` for monitor configuration
- `waybar` for status bar
- `foot` or another Wayland terminal
- `rofi` (with `-normal-window` flag) or `fuzzel` for app launcher
- `wpctl` (PipeWire) for volume control
- `playerctl` for media playback control

## Building

```sh
git clone https://github.com/Marenz/notion-river
cd notion-river
cargo build --release
cp target/release/notion-river ~/.local/bin/
```

## Setup

1. Create the River init script at `~/.config/river/init`:
```sh
#!/bin/sh
export XKB_DEFAULT_LAYOUT=us  # adjust to your layout
export XDG_CURRENT_DESKTOP=river
export MOZ_ENABLE_WAYLAND=1
export RUST_LOG=info

kanshi &  # monitor configuration

(sleep 3
    waybar &
    nm-applet --indicator &
) &

# Restart loop: WM restarts on clean exit (Super+Shift+R)
while notion-river; do
    sleep 0.5
done
```

2. Create the config at `~/.config/notion-river/config.toml`:
```toml
active_profile = "notion"

[general]
physical_keys = true
focus_follows_mouse = true
cursor_follows_focus = true
gap = 4
border_width = 2

[commands]
terminal = "foot"
launcher = ["rofi", "-show", "combi", "-normal-window"]

[appearance]
active_border = "#4c7899"
inactive_border = "#333333"

[[workspaces]]
name = "main"
output = "HDMI-A-1"
initial_layout = "hsplit"

[[workspaces]]
name = "secondary"
output = "HDMI-A-1"

[[workspaces]]
name = "social"
output = "DP-1"
```

3. Start from a TTY:
```sh
river -c ~/.config/river/init
```

## Keybindings

### notion profile (Vim-style)

| Binding | Action |
|---|---|
| `Super+Return` | Terminal |
| `Super+p` | Launcher |
| `Super+c` | Close window / unsplit empty frame |
| `Super+h/j/k/l` | Focus left/down/up/right |
| `Super+Shift+h/j/k/l` | Move window (across monitors too) |
| `Super+s` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split orientation |
| `Super+x` | Remove empty frame |
| `Super+Tab` / `Shift+Tab` | Next / previous tab |
| `Super+1..6` | Switch workspace |
| `Super+Shift+R` | Restart WM (preserves windows) |

### i3_neo profile (Neo layout)

| Binding | Action |
|---|---|
| `Super+Space` | Terminal |
| `Super+o` | Launcher |
| `Super+c` | Close / unsplit |
| `Super+i/a/l/e` | Focus (Neo directions) |
| `Super+Shift+i/a/l/e` | Move window |
| `Super+b` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split |
| `Super+n/p` | Next / previous tab |
| `Super+1..4` | Workspaces (primary monitor) |
| `Alt+1..3` | Workspaces (secondary monitor) |

### Mouse

- `Mod+Left-drag` — move window to another frame
- `Mod+Right-drag` — resize split boundaries
- Click tab bar to switch tabs
- Focus follows mouse

### Media keys

Volume up/down/mute, mic mute, play/pause, next/prev, brightness up/down.

## Status

Early development. Usable as a daily driver. Waybar shows workspaces, CPU, memory, disk, volume, network, and system tray.

### Planned

- Drag-and-drop with visual split preview
- IPC command socket for scripting / clickable waybar
- Window rules (winprops) for auto-placement
- Better font rendering in tab bars

## License

MIT — see [LICENSE](LICENSE)
