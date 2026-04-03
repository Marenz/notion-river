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
- **Hotplug support** — output profiles remember workspace assignments
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
- `kanshi` or `wlr-randr` for monitor configuration
- `waybar` for status bar
- `foot` or another Wayland terminal
- `rofi` for app launcher / window switcher

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

kanshi &

(sleep 3; waybar &; nm-applet --indicator &) &

while true; do
    notion-river
    sleep 0.5
done
```

### Output Change Hook

If `~/.config/notion-river/hooks/on-outputs-changed` exists and is executable,
`notion-river` runs it whenever the current output layout changes and stabilizes.

The hook receives JSON on stdin describing the current outputs:

```json
{
  "outputs": [
    {
      "name": "DP-7",
      "x": 3840,
      "y": 0,
      "width": 1440,
      "height": 2560,
      "usable_x": 3840,
      "usable_y": 0,
      "usable_width": 1440,
      "usable_height": 2560,
      "scale": 1.5,
      "wl_scale": 2,
      "physical_width": 2160,
      "physical_height": 3840
    }
  ]
}
```

The environment variable `NOTION_RIVER_HOOK=outputs-changed` is also set.

This is intended for integrations such as updating `kanshi` profiles after live
monitor changes.

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
