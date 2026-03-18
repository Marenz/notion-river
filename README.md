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
- **App-to-frame bindings** — bind apps to specific frames (`Super+f` toggle, `Super+Shift+f` exclusive), with wildcard app_id matching and optional fixed dimensions
- **Visual binding indicator** — bound frames show ⊙ in the tab bar
- **Auto-float popups** — windows with DimensionsHint auto-float as popups
- **Fullscreen toggle** — `Super+Return` toggles fullscreen
- **Resize mode** — `Super+R` enters/exits, absolute direction semantics (Up always moves boundary up)
- **Focus-follows-mouse** across frames and monitors (including empty frames)
- **Cursor-follows-focus** on keyboard navigation
- **Cross-monitor focus and window moving** with edge-position matching
- **Pointer drag** — left-drag moves windows between frames (drags clicked tab, not active window), right-drag resizes splits
- **Multi-monitor** with per-output workspace assignment
- **Layer-shell support** — waybar, notifications, rofi overlays
- **Waybar integration** — workspace indicators with per-monitor grouping
- **IPC control socket** — `notion-ctl` can list/focus windows, switch workspaces, bind/unbind apps, set fixed dimensions
- **Window switcher** — rofi integration with combined drun+run+windows mode via `notion-ctl`
- **HiDPI / fractional scaling** — wp_viewporter protocol, scale 1.5x via kanshi (clean fraction, no wlroots blur)
- **XWayland support** — River rebuilt with `-Dxwayland` for legacy X11 apps (Steam, etc.)
- **Cairo+Pango font rendering** — crisp tab bar labels
- **Media keys** — volume, brightness, playback controls
- **Physical key bindings** — work across keyboard layouts (Neo, Dvorak, etc.)
- **Runtime keyboard layout switching** — `Ctrl+F12` toggles between de/neo and de
- **Two built-in keybinding profiles** — `i3_neo` (Neo layout) and `notion` (Vim-style)
- **State persistence** — layout, window placement, active tabs, and app bindings saved in `~/.config/notion-river/` (survives reboots). Identifier-only restore matching.
- **Split moves active window** — splitting a multi-tab frame moves the current window to the new frame
- **TOML configuration**

## Requirements

- [River](https://codeberg.org/river/river) 0.4.x+ (built from source with `-Dxwayland=true`, uses `river-window-management-v1` protocol)
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

# Restart loop: WM always restarts (Super+Shift+R or crash)
while true; do
    notion-river
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
| `Super+Return` | Fullscreen toggle |
| `Super+p` | Launcher |
| `Super+Shift+p` | Window switcher |
| `Super+c` | Close window / unsplit empty frame |
| `Super+h/j/k/l` | Focus left/down/up/right |
| `Super+Shift+h/j/k/l` | Move window (across monitors too) |
| `Super+s` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split orientation |
| `Super+x` | Remove empty frame |
| `Super+Tab` / `Shift+Tab` | Next / previous tab |
| `Super+1..6` | Switch workspace |
| `Super+f` | Toggle app binding to current frame |
| `Super+Shift+f` | Exclusive app binding to current frame |
| `Super+R` | Enter / exit resize mode |
| `Super+Shift+R` | Restart WM (preserves windows) |
| `Ctrl+F12` | Toggle keyboard layout (de/neo ↔ de) |

### i3_neo profile (Neo layout)

| Binding | Action |
|---|---|
| `Super+Space` | Terminal |
| `Super+o` | Launcher |
| `Super+Shift+o` | Window switcher |
| `Super+c` | Close / unsplit |
| `Super+i/a/l/e` | Focus (Neo directions) |
| `Super+Shift+i/a/l/e` | Move window |
| `Super+b` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split |
| `Super+n/p` | Next / previous tab |
| `Super+1..4` | Workspaces (primary monitor) |
| `Alt+1..3` | Workspaces (secondary monitor) |
| `Super+f` | Toggle app binding to current frame |
| `Super+Shift+f` | Exclusive app binding to current frame |
| `Super+R` | Enter / exit resize mode |
| `Super+Return` | Fullscreen toggle |
| `Ctrl+F12` | Toggle keyboard layout (de/neo ↔ de) |

### Resize mode

Enter with `Super+R`. Arrow keys move split boundaries in absolute directions (Up always moves the boundary up, Down always moves it down, etc. — no relative-to-split-side confusion). Press `Super+R` again or `Escape` to exit.

### Mouse

- `Mod+Left-drag` — move window to another frame (drags the clicked tab, not the active one)
- `Mod+Right-drag` — resize split boundaries
- Click tab bar to switch tabs
- Focus follows mouse

### Media keys

Volume up/down/mute, mic mute, play/pause, next/prev, brightness up/down.

## App Bindings

Bind an app to a specific frame so it always opens there:

- **`Super+f`** on a focused window toggles a binding between that app and the current frame
- **`Super+Shift+f`** creates an exclusive binding (frame only accepts that app)
- Bindings persist in `~/.config/notion-river/bindings.json` (survives reboots)
- **Wildcard matching**: `steam_app_*` matches all Steam game windows
- **Fixed dimensions**: set a fixed resolution per binding (e.g. 1920x1080 for Steam streaming) — the window is forced to that size regardless of frame geometry
- **Visual indicator**: bound frames show ⊙ in the tab bar
- **Auto-enforcement**: `enforce_app_bindings` runs each manage cycle, automatically moving bound windows to the correct frame on the visible workspace

### IPC binding commands

```sh
notion-ctl bind <app_id> <workspace> <frame_path>
notion-ctl unbind <app_id>
notion-ctl set-fixed-dimensions <app_id> <width>x<height>
```

## Status

Usable as a daily driver. App-to-frame bindings with wildcard matching and fixed dimensions. XWayland support for legacy apps. IPC control socket enables external tooling (rofi window switcher, app binding management). Tab bars use Cairo+Pango for crisp font rendering. HiDPI works at scale 1.5x via kanshi (clean fraction, no wlroots blur).

### Planned

- Drag-and-drop with visual split preview
- Window rules (winprops) for auto-placement

## IPC

The WM exposes a Unix socket at `$XDG_RUNTIME_DIR/notion-river.sock`.

Commands:

```sh
notion-ctl list-windows
notion-ctl list-workspaces
notion-ctl focus-window <id>           # switches to hidden workspace if needed
notion-ctl switch-workspace <name>
notion-ctl bind <app_id> <ws> <path>   # bind app to frame
notion-ctl unbind <app_id>             # remove binding
notion-ctl set-fixed-dimensions <app_id> <w>x<h>  # fixed window size for binding
```

Window switcher helper (rofi with combined drun+run+windows mode):

```sh
notion-rofi-windows
```

This is bound to `Super+Shift+o` (`i3_neo`) and `Super+Shift+p` (`notion`).

## License

MIT — see [LICENSE](LICENSE)
