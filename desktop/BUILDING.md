# Building arbos-desktop on Linux

The desktop is a [gpui](https://www.gpui.rs) app: it links against X11/xcb,
xkbcommon, Wayland, fontconfig/FreeType, Vulkan, ALSA, udev and D-Bus, and one
of its dependencies' build scripts links `-lstdc++`. A stock Ubuntu 24.04 has
none of the `-dev` halves and no C++ toolchain, so `cargo build` stops at the
first missing header, and — once those are in — at the link step with nothing
but `linker command failed` unless the verbose output is read. This page is
the list a new machine needs, checked on the rig that builds this app most
often (Ubuntu 24.04, Rust nightly per `rust-toolchain.toml`).

## Packages (Debian/Ubuntu names)

```bash
sudo apt install build-essential pkg-config clang cmake libssl-dev libasound2-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev \
  libxcb1-dev libfontconfig1-dev libfreetype6-dev libvulkan-dev libgl1-mesa-dev \
  libegl1-mesa-dev libudev-dev libdbus-1-dev
```

What each group is for, so a different distribution can map the names:

| need | packages | why |
| --- | --- | --- |
| C and C++ toolchain | `build-essential` (`gcc`, `g++`, `make`), `clang`, `cmake`, `pkg-config` | `g++` brings `libstdc++-13-dev`, which owns the unversioned `libstdc++.so` that `-lstdc++` resolves to; without it the runtime `libstdc++6` alone is not enough and the link fails |
| windowing | `libx11-dev`, `libx11-xcb-dev`, `libxcb1-dev`, `libxkbcommon-dev`, `libxkbcommon-x11-dev`, `libwayland-dev` | gpui's X11 and Wayland backends |
| text | `libfontconfig1-dev`, `libfreetype6-dev` | font discovery and rasterisation |
| GPU | `libvulkan-dev`, `libgl1-mesa-dev`, `libegl1-mesa-dev` | the renderer; at run time a Vulkan driver (`mesa-vulkan-drivers` for lavapipe on a headless box) |
| audio, devices, desktop bus | `libasound2-dev`, `libudev-dev`, `libdbus-1-dev` | microphone and speaker, device events, notifications |
| TLS | `libssl-dev` | the kernel and the desktop's HTTP clients |

CI installs the same set on `ubuntu-latest` (`.github/workflows/ci.yml`),
where `build-essential` is preinstalled — which is why the list in the README
did not need it there and a fresh machine did.

## The `-lstdc++` link failure

Symptom, after every header is in:

```
= note: rust-lld: error: unable to find library -lstdc++
error: linking with `cc` failed
```

Cause: `libstdc++.so` (the linker-time symlink) lives in
`/usr/lib/gcc/x86_64-linux-gnu/13/` and is owned by `libstdc++-13-dev`. The
runtime package `libstdc++6` ships only `libstdc++.so.6`, which `-lstdc++`
does not match. The search path is added by the `cc` driver only when a GCC
installation is present.

Fix: `sudo apt install g++` (part of `build-essential` above). If the machine
cannot take a compiler, the same directory can be handed to the linker
directly for one build:

```bash
RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13" cargo build --release
```

(`13` is Ubuntu 24.04's GCC; `ls /usr/lib/gcc/x86_64-linux-gnu/` shows the
version on another release.)

## Checking the binary is the tree's

`arbos-desktop --version` prints `<version> <build> <sha>[-dirty]`, for
example `0.2.0 1462 42cb975-dirty`. A `cargo build` that fails leaves the
previous binary in `target/`; anything that copies and launches it afterwards
runs an old build without saying so. The parity rig refuses to launch a
binary whose sha is not the checkout's HEAD (`qa/parity/arbos-launch.sh`,
`qa/parity/ui_pass.py`, `qa/parity/journey.py`); a script of your own should
do the same before it trusts a result.

## Headless machines

The rig runs the app on an Xvfb/TigerVNC display (`DISPLAY=:1`) with
`mesa-vulkan-drivers` for a software Vulkan device, `xdotool` and `wmctrl`
for window placement, `scrot` for stills, and `dunst` so OS notifications can
be read back with `dunstctl history`. Set `ARBOS_DRIVER_SOCKET` to a path to
turn on the driver socket the rig speaks JSON over (`qa/parity/arbosdriver.py`).
