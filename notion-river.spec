Name:           notion-river
Version:        0.1.0
Release:        1%{?dist}
Summary:        Notion/Ion3-style static tiling window manager for River
License:        MIT
URL:            https://github.com/Marenz/notion-river
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.75
BuildRequires:  rust >= 1.75
BuildRequires:  pkg-config
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(cairo)
BuildRequires:  pkgconfig(cairo-ft)
BuildRequires:  pkgconfig(pango)
BuildRequires:  pkgconfig(pangocairo)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(freetype2)

Requires:       libwayland-client0
Requires:       libcairo2
Requires:       libpango-1_0-0
Requires:       libpangocairo-1_0-0
Requires:       libxkbcommon0
Requires:       libfreetype6

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

%install
install -Dm755 target/release/notion-river %{buildroot}%{_bindir}/notion-river
install -Dm755 target/release/notion-ctl %{buildroot}%{_bindir}/notion-ctl
install -Dm644 notion-river.desktop %{buildroot}%{_datadir}/wayland-sessions/notion-river.desktop
install -dm755 %{buildroot}%{_datadir}/notion-river/examples
install -Dm755 config-examples/start-river %{buildroot}%{_datadir}/notion-river/examples/start-river
install -Dm755 config-examples/notion-rofi-window-mode %{buildroot}%{_datadir}/notion-river/examples/notion-rofi-window-mode

%files
%license LICENSE
%doc README.md config.example.toml
%{_bindir}/notion-river
%{_bindir}/notion-ctl
%{_datadir}/wayland-sessions/notion-river.desktop
%{_datadir}/notion-river/

%changelog
* Wed Mar 18 2026 Marenz <marenz@users.noreply.github.com> - 0.1.0-1
- Initial package
