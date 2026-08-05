Name:           river
Version:        0.4.7
Release:        1%{?dist}
Summary:        Dynamic tiling Wayland compositor

License:        GPLv3+
URL:            https://codeberg.org/river/river

# River 0.4.7 requires zig 0.16, which openSUSE does not package (Tumbleweed and
# OBS devel:languages:zig both stop at 0.15.2). There is therefore no zig
# BuildRequires: point %{zig} at an upstream toolchain instead, e.g.
#   rpmbuild --define 'zig %{_builddir}/../zig-0.16.0/zig' ...
# It defaults to whatever "zig" is on PATH.
%{!?zig: %global zig zig}

BuildRequires:  pkg-config
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(wayland-protocols)
BuildRequires:  pkgconfig(wlroots-0.20)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(libevdev)
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(pixman-1)
BuildRequires:  libXcursor-devel

%description
River is a dynamic tiling Wayland compositor with flexible runtime
configuration. This build includes XWayland support for running
X11 applications.

%prep
# Source is provided via git submodule in vendor/river

%build
cd vendor/river
%{zig} build -Doptimize=ReleaseSafe -Dxwayland

%install
# River 0.4.x has no riverctl; configuration moved to the window management
# protocol, which notion-river implements.
install -Dm755 vendor/river/zig-out/bin/river %{buildroot}%{_bindir}/river

%files
%{_bindir}/river
