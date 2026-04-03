Name:           notion-river
Version:        %{?version}%{!?version:0.4.1}
Release:        1%{?dist}
Summary:        Notion/Ion3-style static tiling window manager for River
License:        MIT
URL:            https://github.com/Marenz/notion-river
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  (cargo >= 1.75 or rustup)
BuildRequires:  pkg-config
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(cairo)
BuildRequires:  pkgconfig(cairo-ft)
BuildRequires:  pkgconfig(pango)
BuildRequires:  pkgconfig(pangocairo)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(freetype2)

# Runtime deps are auto-detected from shared library linkage

Recommends:     foot
Recommends:     waybar
Recommends:     rofi-wayland

%description
notion-river is a window manager for the River Wayland compositor that
implements static tiling from the Notion WM. The screen layout is a
persistent wireframe of frames that exist independently of windows.
Windows are placed into frames as tabs. Opening/closing windows never
changes the layout — only explicit user actions (split/unsplit) do.

Requires the River compositor (0.4.x+) to be installed separately.

%prep
%autosetup

%build
cargo build --release
# Build River from vendored source
cd vendor/river
zig build -Doptimize=ReleaseSafe -Dxwayland || zig build -Doptimize=ReleaseSafe || true
cd ../..

%install
install -Dm755 target/release/notion-river %{buildroot}%{_bindir}/notion-river
install -Dm755 target/release/notion-ctl %{buildroot}%{_bindir}/notion-ctl
install -Dm755 notion-river-session %{buildroot}%{_bindir}/notion-river-session
install -Dm755 config-examples/notion-rofi-window-mode %{buildroot}%{_bindir}/notion-rofi-window-mode
install -Dm755 config-examples/notion-volume %{buildroot}%{_bindir}/notion-volume
install -Dm755 config-examples/notion-cycle-workspace %{buildroot}%{_bindir}/notion-cycle-workspace
# Include River if built
test -f vendor/river/zig-out/bin/river && install -Dm755 vendor/river/zig-out/bin/river %{buildroot}%{_bindir}/river || true
install -Dm644 notion-river.desktop %{buildroot}%{_datadir}/wayland-sessions/notion-river.desktop
install -dm755 %{buildroot}%{_datadir}/notion-river/examples
install -Dm644 config-examples/start-river %{buildroot}%{_datadir}/notion-river/examples/start-river
install -Dm644 config-examples/kanshi.service %{buildroot}%{_datadir}/notion-river/examples/kanshi.service
install -Dm755 config-examples/river-init %{buildroot}%{_datadir}/notion-river/examples/river-init
install -Dm644 config-examples/autostart %{buildroot}%{_datadir}/notion-river/examples/autostart
install -Dm644 config.example.toml %{buildroot}%{_datadir}/notion-river/examples/config.toml
install -dm755 %{buildroot}%{_datadir}/notion-river/examples/waybar
install -Dm644 config-examples/waybar/config.jsonc %{buildroot}%{_datadir}/notion-river/examples/waybar/config.jsonc
install -Dm644 config-examples/waybar/style.css %{buildroot}%{_datadir}/notion-river/examples/waybar/style.css

%files
%license LICENSE
%doc README.md config.example.toml
%{_bindir}/notion-river
%{_bindir}/notion-ctl
%{_bindir}/notion-river-session
%{_bindir}/notion-rofi-window-mode
%{_bindir}/notion-volume
%{_bindir}/notion-cycle-workspace
%ghost %{_bindir}/river
%{_datadir}/wayland-sessions/notion-river.desktop
%{_datadir}/notion-river/

%changelog
* Sun Mar 29 2026 Marenz <marenz@users.noreply.github.com> - 0.5.1-1
- Lock resize to grabbed boundary (no more jumping in 3+ way splits)
- Per-axis corner resize (both H and V boundaries adjusted simultaneously)
- Visual resize highlight: configurable semi-transparent line on grabbed boundary
- Reorder render cycle to eliminate resize highlight lag
- Auto-focus and warp cursor to newly positioned floating windows
- Click floating notification switches to parent app's workspace
- Fix app binding retabbing and show bind markers per tab

* Tue Mar 24 2026 Marenz <marenz@users.noreply.github.com> - 0.5.0-1
- Fix resize lag: pointer now tracks 1:1 during RMB resize
- Fix floating window ghost titlebar on first appearance
- Fix late-float race: dialogs no longer appear as tabs in tiled frames
- Remove stale IPC socket on startup (fixes workspace switching after restart)
- Log rotation: previous run preserved as .prev for crash investigation
- New: subscribe-output IPC command for dynamic per-output workspace modules
- Updated waybar examples with GPU, VRAM, temperature, disk modules
- Auto-install example configs on first run
- Per-output waybar config (portrait screen optimization)

* Sun Mar 22 2026 Marenz <marenz@users.noreply.github.com> - 0.4.1-1
- Fix clippy lint, fix packaging

* Sun Mar 22 2026 Marenz <marenz@users.noreply.github.com> - 0.4.0-1
- Rounded top corners on every individual tab with transparent gaps
- Configurable tab gradients (focused, unfocused active, inactive)
- Split init script: generic river-init + user-specific autostart
- Import Wayland env into systemd for portal support (file dialogs)

* Wed Mar 18 2026 Marenz <marenz@users.noreply.github.com> - 0.1.0-1
- Initial package
