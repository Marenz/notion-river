Name:           river
Version:        0.4.0
Release:        1%{?dist}
Summary:        Dynamic tiling Wayland compositor

License:        GPLv3+
URL:            https://codeberg.org/river/river

BuildRequires:  zig >= 0.13
BuildRequires:  pkg-config
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(wayland-protocols)
BuildRequires:  pkgconfig(wlroots-0.19)
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
zig build -Doptimize=ReleaseSafe -Dxwayland

%install
install -Dm755 vendor/river/zig-out/bin/river %{buildroot}%{_bindir}/river
install -Dm755 vendor/river/zig-out/bin/riverctl %{buildroot}%{_bindir}/riverctl 2>/dev/null || true

%files
%{_bindir}/river
