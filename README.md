<h1 align="center">notion-river</h1>

<p align="center">
  <b>Static tiling window manager for <a href="https://codeberg.org/river/river">River</a></b><br>
  <i>Inspired by <a href="https://notionwm.net/">Notion</a> (formerly Ion3)</i>
</p>

<p align="center">
  <a href="#features">Features</a> &bull;
  <a href="#how-it-works">How it Works</a> &bull;
  <a href="#getting-started">Getting Started</a> &bull;
  <a href="#keybindings">Keybindings</a> &bull;
  <a href="#ipc">IPC</a> &bull;
  <a href="#configuration">Configuration</a>
</p>

---

## How it Works

Unlike dynamic tiling WMs (i3, Sway, Hyprland) where the layout reflows every time a window opens or closes, notion-river uses **persistent frames**:

```
 ┌──────────────┬──────────┐
 │              │  Browser  │
 │   Terminal   ├──────────┤
 │              │  Editor   │
 ├──────────────┤  (tab 2)  │
 │   (empty)    │           │
 └──────────────┴──────────┘
```

- **Frames** are the skeleton of your workspace — they exist independently of windows
- Windows live *inside* frames as **tabs** (multiple windows per frame, one visible at a time)
- Opening or closing a window **never changes the layout** — only your explicit split/unsplit commands do
- Empty frames are visible as wireframe outlines, ready for new windows

The result: a predictable, stable workspace that doesn't rearrange itself.

## Features

### Tiling
- **Static split tree** — manual horizontal/vertical splits with adjustable ratios
- **Tabbed frames** — multiple windows per frame, click tab bar or `Super+n/p` to switch
- **Empty frame indicators** — visible wireframe cells waiting for windows
- **Cross-monitor focus & move** — seamless window movement between outputs with edge-position matching
- **Resize mode** — `Super+R` enters resize mode with absolute direction semantics

### Floating
- **Auto-float dialogs** — secondary windows from bound apps float automatically
- **Auto-float notifications** — untitled popups (e.g. Thunderbird) float in the top-right corner
- **Drag to move** — `Super+LMB` moves floating windows
- **Focus-follows-mouse** — hover over a floating window to focus it
- **Keyboard control** — `Super+C` closes the focused floating window
- **Borders** — floating windows get a colored border matching your theme

### App Bindings
- **Bind apps to frames** — `Super+F` toggles, `Super+Shift+F` makes exclusive
- **Wildcard matching** — `steam_app_*` binds all Steam games to one frame
- **Fixed dimensions** — force a resolution per binding (e.g. 1920x1080 for game streaming)
- **Auto-enforcement** — bound windows are moved to the correct frame automatically
- **Visual indicator** — bound frames show `⊙` in the tab bar

### Waybar Integration
- **Event-driven** — zero-polling workspace modules via IPC subscriptions
- **Per-workspace click** — click a workspace name to switch to it
- **Configurable appearance** — decoration colors from `config.toml`

### Pointer
- **Drag & drop** — `Super+LMB` moves windows between frames with visual split preview
- **Resize splits** — `Super+RMB` adjusts split boundaries
- **Tab-specific drag** — clicking a non-active tab and dragging moves *that* tab
- **Focus-follows-mouse** — works across frames, monitors, and floating windows

### Multi-Monitor
- **Per-output workspaces** — each workspace assigned to a preferred output
- **Hotplug support** — per-monitor memory (EDID-keyed) remembers which workspace was last shown on each physical monitor
- **Graceful disconnect** — workspaces stay intact when a monitor disconnects
- **Automatic restore** — reconnecting monitors restores previous layout

### Other
- **State persistence** — layout, windows, tabs, bindings survive reboots
- **IPC control socket** — `notion-ctl` for scripting and rofi integration
- **HiDPI** — Cairo+Pango rendering, wp_viewporter, clean 1.5x scaling
- **XWayland** — support for legacy X11 apps (Steam, etc.)
- **Physical key bindings** — work across keyboard layouts (Neo, Dvorak)
- **Media keys** — volume, brightness, playback controls
- **Configurable appearance** — tab bar colors, borders, underlines via TOML

## Getting Started

### Requirements

- [River](https://codeberg.org/river/river) 0.4.x+ (uses `river-window-management-v1` protocol)
- Rust 1.75+
- `waybar` for status bar
- `foot` or another Wayland terminal
- `rofi` for app launcher / window switcher
- `wdisplays` (optional) for one-shot interactive monitor layout edits — notion-river itself owns and remembers the layout via `wlr-output-management-unstable-v1`. Do **not** run kanshi or another wlr-output-management client alongside notion-river.

### Building

```sh
git clone https://github.com/Marenz/notion-river
cd notion-river
cargo build --release
cp target/release/notion-river target/release/notion-ctl ~/.local/bin/
```

### Setup

1. **River init script** at `~/.config/river/init`:

```sh
#!/bin/sh
export XKB_DEFAULT_LAYOUT=us
export XDG_CURRENT_DESKTOP=river
export MOZ_ENABLE_WAYLAND=1
export RUST_LOG=info

(sleep 3; waybar &; nm-applet --indicator &) &

while true; do
    notion-river
    sleep 0.5
done
```

### Rofi launcher & window switcher

The primary entry point is one unified box (`Super+o`) that switches to an
open window or launches an app:

```toml
[commands]
# Super+o — unified switch-or-launch
launcher = ["notion-rofi-launch"]
# Super+Shift+o — same menu, bound separately for muscle memory
window_switcher = ["notion-rofi-window-switch"]
```

- **`notion-rofi-launch`** shows the `notion-rofi-window-mode` script-modi,
  which owns the whole list: open windows alongside launchable apps. Type a
  name to jump to a running window, or launch the app if it is not open.
  Picks are focused via `notion-ctl focus-window` (switching workspaces if the
  window is hidden) and apps are started via `gtk-launch`.
- **`notion-rofi-window-switch`** opens the same menu; a windows-only switcher
  would be a strict subset of it.

This works with **stock rofi** and needs no foreign-toplevel protocol, which
is deliberate. River keeps focus decisions in the window manager, so rofi's
built-in `window` modi cannot be used:

- `ext-foreign-toplevel-list-v1`, which River serves, is list-only: it can
  enumerate windows but not activate them.
- River also serves `zwlr_foreign_toplevel_manager_v1`, but only a subset that
  reports title/app_id/activated state and ignores the `activate` request.
  rofi prefers that protocol whenever it is advertised — it binds both and
  then destroys the `ext-foreign-toplevel-list` object — so the built-in modi
  would list windows and then silently fail to focus them.

rofi's `combi` modi is avoided for a separate reason: it swallows the
script-modi selection callback, so a single custom modi that owns the whole
list is the clean solution.

Both wrappers target the focused output by name (`-m <output>`): notion-river
reports every output at position `0,0` to layer-shell clients, so coordinate
placement does not work and rofi would otherwise always open on the same
monitor.

The window switcher and its `notion-rofi-window-mode` script-modi, the launcher,
and `notion-ctl` are all installed to `PATH` by the packages. The optional
Catppuccin Mocha rofi theme is shipped under the examples directory:

```sh
mkdir -p ~/.config/rofi
cp /usr/share/notion-river/examples/rofi/*.rasi ~/.config/rofi/
```

Edit `~/.config/rofi/config.rasi` to set your preferred `font` and
`icon-theme`.

### Monitor configuration

notion-river owns monitor layout (mode, position, scale, transform) directly
via `wlr-output-management-unstable-v1` and persists it to
`~/.config/notion-river/monitors.json`, keyed by the sorted set of EDID
descriptors of the connected monitors.

Behaviour:

- First time a given set of monitors is seen: whatever the compositor picked
  is recorded as the initial profile.
- Subsequent appearances of the same set: the saved profile is applied
  instantly (no flicker, no waiting).
- Edit the layout interactively with `wdisplays` (or any other
  wlr-output-management client) and notion-river will save the new layout
  for that set immediately.
- Suspend/resume preserves layout — the EDID set doesn't change, so no apply
  is triggered.

Do not run kanshi or another wlr-output-management client; two clients
fighting over the same protocol breaks layout persistence.

2. **WM config** at `~/.config/notion-river/config.toml`:

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
active_border = "#cba6f7"
inactive_border = "#1e1a2e"
tab_focused_active = "#5b4a8a"
tab_active = "#3b2d5e"
tab_inactive = "#1e1a2e"
tab_underline_focused = "#cba6f7"
tab_text_active = "#f5f0ff"
tab_text_inactive = "#9085a8"

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

3. **Start from a TTY**:

```sh
river -c ~/.config/river/init
```

## Keybindings

### `notion` profile (Vim-style)

| Binding | Action |
|---|---|
| `Super+Return` | Fullscreen toggle |
| `Super+p` | Launcher |
| `Super+Shift+p` | Window switcher |
| `Super+c` | Close window / unsplit empty frame |
| `Super+h/j/k/l` | Focus left/down/up/right |
| `Super+Shift+h/j/k/l` | Move window (cross-monitor) |
| `Super+s` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split orientation |
| `Super+x` | Remove empty frame |
| `Super+Tab` / `Shift+Tab` | Next / previous tab |
| `Super+1..6` | Switch workspace |
| `Super+f` | Toggle app binding |
| `Super+Shift+f` | Exclusive app binding |
| `Super+R` | Enter / exit resize mode |
| `Super+Shift+R` | Restart WM (preserves windows) |

### `i3_neo` profile (Neo layout)

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
| `Super+n/p` | Next / previous tab |
| `Super+1..4` | Workspaces (primary) |
| `Alt+1..3` | Workspaces (secondary) |

### Mouse

| Binding | Action |
|---|---|
| `Super+LMB drag` | Move window (tiled: between frames with preview; floating: reposition) |
| `Super+RMB drag` | Resize split boundaries |
| Click tab bar | Switch tab |
| Hover | Focus follows mouse |

### Resize mode

Enter with `Super+R`. Arrow keys move split boundaries in absolute directions (Up always moves the boundary up, regardless of which side of the split you're on). `Super+R` or `Escape` to exit.

## IPC

Unix socket at `$XDG_RUNTIME_DIR/notion-river.sock`. Use `notion-ctl`:

```sh
notion-ctl list-windows                         # JSON list of all windows
notion-ctl list-workspaces                      # JSON list of workspaces
notion-ctl focus-window <id>                    # Focus window (switches workspace if needed)
notion-ctl switch-workspace <name>              # Switch to workspace
notion-ctl subscribe-workspaces                 # Stream all workspace state changes (waybar)
notion-ctl subscribe-workspace <name>           # Stream single workspace state (waybar)
notion-ctl bind <app_id> <ws> <frame> [WxH]    # Bind app to frame
notion-ctl unbind <app_id>                      # Remove binding
notion-ctl set-fixed-dimensions <app_id> <WxH>  # Fixed window size
```

### Event-driven waybar

Instead of polling, waybar modules use `subscribe-workspace` for zero-overhead updates:

```jsonc
"custom/ws-main": {
    "exec": "notion-ctl subscribe-workspace main",
    "return-type": "json",
    "restart-interval": 3,
    "on-click": "notion-ctl switch-workspace main"
}
```

## Configuration

### Appearance

All tab bar and border colors are configurable in `config.toml` under `[appearance]`:

| Key | Description |
|---|---|
| `active_border` | Border color for focused frame |
| `inactive_border` | Border color for unfocused frames |
| `tab_focused_active` | Tab background when focused + active |
| `tab_active` | Tab background when active but unfocused |
| `tab_inactive` | Tab background for non-active tabs |
| `tab_separator` | Color between tabs |
| `tab_underline_focused` | Underline on active tab (focused) |
| `tab_underline_unfocused` | Underline on active tab (unfocused) |
| `tab_text_active` | Text color for active tab |
| `tab_text_inactive` | Text color for inactive tabs |
| `empty_focused` | Empty frame indicator (focused) |
| `empty_unfocused` | Empty frame indicator (unfocused) |
| `monitor_colors` | Per-monitor accent colors for waybar |

## License

MIT — see [LICENSE](LICENSE)
