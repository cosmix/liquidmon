# Stack & Dependencies

> Project technology stack, frameworks, and key dependencies.
> This file is append-only - agents add discoveries, never delete.

## Project Type

- **Rust** (Cargo.toml found)

## Rust Toolchain and Edition

- Rust edition 2024 (`Cargo.toml:4`)
- Package: `liquidmon` v0.1.0, license MPL-2.0

## Direct Dependencies

### `libcosmic` (git = "https://github.com/pop-os/libcosmic.git", rev = "564ef834cec33a948dc10c9b401cf29db5d18373")

The core UI and application framework from System76/Pop!_OS. Provides the `cosmic::Application` trait, widget toolkit (iced-based), applet runtime, config system, and theming. Pinned to a specific rev for reproducibility.

**Features enabled:**

- `applet` — Activates the panel-applet mode of the COSMIC app framework; required for `cosmic::applet::run` and `core.applet.*` helpers
- `applet-token` — Provides the applet token mechanism used by the COSMIC compositor to identify and position panel applets
- `dbus-config` — Hooks into `cosmic-settings-daemon` to watch for config file changes; enables `core.watch_config::<Config>()` subscription
- `multi-window` — Enables the popup/secondary-window system used for the hover popup
- `tokio` — Uses Tokio as the async executor for the COSMIC runtime; required because the app spawns async tasks (liquidctl polling)
- `wayland` — Adds Wayland window/surface support via winit; required on Pop!_OS/COSMIC which is Wayland-only
- `winit` — Windowing backend; provides the event loop and window management layer

### `tokio` v1.x, features = ["process", "time"]

Async runtime. Used for `tokio::process::Command` to spawn `liquidctl` as a subprocess (`liquidctl.rs:9,116`) and `tokio::time::sleep` / `tokio::time::timeout` for the 1500 ms polling interval and 3 s subprocess timeout (`app.rs:235`, `liquidctl.rs:119`). Features narrowed from `["full"]` to only the two features actually used.

### `serde` v1.0.228, features = ["derive"]

Serialization framework. Used to derive `Deserialize` on `DeviceEntry` and `StatusEntry` (`liquidctl.rs:95,105`) to parse liquidctl JSON output.

### `serde_json` v1.0.149

JSON parsing. Used in `liquidctl.rs:143` to deserialize the raw liquidctl `--json status` output into `Vec<DeviceEntry>`. Also the type of `StatusEntry::value` (`serde_json::Number`, `liquidctl.rs:108`).

### `futures-util` v0.3.31

Async stream utilities. Provides `SinkExt` (imported at `app.rs:14`) for `.send()` on the mpsc channel inside the liquidctl subscription stream, and `futures_util::future::pending()` for the infinite-loop sentinel (`app.rs:237`).

## Runtime Dependencies (not in Cargo.toml)

### `liquidctl` (system package)

The Python CLI tool that communicates with the AIO over HID. The applet shells out to `liquidctl --match Hydro --json status` every 1500 ms (`app.rs:229`, `liquidctl.rs:116`). Must be installed via `pip` or system package manager.

### udev rules (`/etc/udev/rules.d/71-liquidctl.rules`)

Required for unprivileged access to `/dev/hidraw*`. Installed by `scripts/install-liquidctl-udev.sh`. Without these rules the applet process must run as root or liquidctl will fail with a permission error.

## Build Tooling

### `just` (justfile)

Task runner. Key targets:

- `build-release` — `cargo build --release` (default)
- `build-debug` — `cargo build`
- `build-vendored` — offline build using vendored deps (for packaging/distro)
- `check` — `cargo clippy --all-features` with pedantic warnings
- `install` — copies binary, `.desktop`, `.metainfo.xml`, and icon SVG to `$prefix` (default `/usr`)
- `uninstall` — removes installed files
- `run` — `RUST_BACKTRACE=full cargo run --release`
- `vendor` / `vendor-extract` — creates/extracts `vendor.tar` for offline builds; `vendor` now includes the `tar pcf vendor.tar vendor .cargo` step to produce the tarball
- `tag <version>` — bumps `Cargo.toml` version, commits, and creates a git tag
- `clean` / `clean-vendor` / `clean-dist` — cleanup targets

## Install Paths (justfile)

Binary: `/usr/bin/liquidmon`
Desktop file: `/usr/share/applications/com.github.cosmix.LiquidMon.desktop`
Metainfo: `/usr/share/appdata/com.github.cosmix.LiquidMon.metainfo.xml`
Icon: `/usr/share/icons/hicolor/scalable/apps/com.github.cosmix.LiquidMon.svg`

## Resources

- `resources/app.desktop` — GNOME/freedesktop desktop entry; `X-CosmicApplet=true` marks it as a COSMIC panel applet; `X-CosmicHoverPopup=Auto` enables the hover popup behavior; `NoDisplay=true` hides it from app launchers
- `resources/app.metainfo.xml` — AppStream/AppData metadata for software centers; component id `com.github.cosmix.LiquidMon`
- `resources/icon.svg` — scalable app icon (referenced in justfile install target)
- `resources/icons/` — embedded SVG icons for temperature, snowflake, fan, and pump symbols (included via `include_bytes!` in `src/app.rs:23-26`)

## COSMIC Desktop Registration

The applet is registered with the COSMIC panel via the `.desktop` file:

- `X-CosmicApplet=true` — tells the COSMIC panel this is an applet
- `X-CosmicHoverPopup=Auto` — enables the hover popup mode
- App ID `com.github.cosmix.LiquidMon` must match `APP_ID` in `app.rs:81`
- The COSMIC panel reads installed `.desktop` files from `/usr/share/applications/` to enumerate available applets
