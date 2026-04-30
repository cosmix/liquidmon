# Stack & Dependencies

> Project technology stack, frameworks, and key dependencies.
> This file is append-only - agents add discoveries, never delete.

(Add stack information as you discover it)

## Project Type

- **Rust** (Cargo.toml found)

## Key Dependencies

### Rust Dependencies (from Cargo.toml)

- futures-util
- i18n-embed-fl
- rust-embed
- serde
- serde_json
- tokio

## Rust Toolchain and Edition

- Rust edition 2024 (`Cargo.toml:4`)
- Package: `cosmic-liquid` v0.1.0, license MPL-2.0

## Direct Dependencies

### `libcosmic` (git = "https://github.com/pop-os/libcosmic.git")

The core UI and application framework from System76/Pop\!\_OS. Provides the `cosmic::Application` trait, widget toolkit (iced-based), applet runtime, config system, and theming. Pinned to git main (no version tag). A local patch path is commented out at `Cargo.toml:41-44` for testing against a local clone.

**Features enabled:**

- `applet` — Activates the panel-applet mode of the COSMIC app framework; required for `cosmic::applet::run` and `core.applet.*` helpers (`app.rs:104`, `app.rs:157`)
- `applet-token` — Provides the applet token mechanism used by the COSMIC compositor to identify and position panel applets
- `dbus-config` — Hooks into `cosmic-settings-daemon` to watch for config file changes; enables `core.watch_config::<Config>()` subscription (`app.rs:187`)
- `multi-window` — Enables the popup/secondary-window system used for the hover popup (`app.rs:5`, `app.rs:221-235`)
- `tokio` — Uses Tokio as the async executor for the COSMIC runtime; required because the app spawns async tasks (liquidctl polling)
- `wayland` — Adds Wayland window/surface support via winit; required on Pop\!\_OS/COSMIC which is Wayland-only
- `winit` — Windowing backend; provides the event loop and window management layer

### `tokio` v1.48, features = ["full"]

Async runtime. Used for `tokio::process::Command` to spawn `liquidctl` as a subprocess (`liquidctl.rs:8,109`) and `tokio::time::sleep` for the 1500 ms polling interval (`app.rs:177`).

### `serde` v1.0.228, features = ["derive"]

Serialization framework. Used to derive `Deserialize` on `DeviceEntry` and `StatusEntry` (`liquidctl.rs:88,98`) to parse liquidctl JSON output.

### `serde_json` v1.0.149

JSON parsing. Used in `liquidctl.rs:134` to deserialize the raw liquidctl `--json status` output into `Vec<DeviceEntry>`. Also the type of `StatusEntry::value` (`serde_json::Number`, `liquidctl.rs:101`).

### `futures-util` v0.3.31

Async stream utilities. Provides `SinkExt` (imported at `app.rs:10`) for `.send()` on the mpsc channel inside the liquidctl subscription stream, and `futures_util::future::pending()` for the infinite-loop sentinel (`app.rs:181`).

### `i18n-embed` v0.16, features = ["fluent-system", "desktop-requester"]

Internationalization embedding framework. `fluent-system` enables Project Fluent (.ftl) file support; `desktop-requester` enables `DesktopLanguageRequester` which reads the system's preferred locale at startup (`main.rs:10`). Used in `i18n.rs`.

### `i18n-embed-fl` v0.10

Compile-time macro companion to `i18n-embed` for Fluent. Provides the `fl\!()` macro used in `i18n.rs:46-51`.

### `rust-embed` v8.7.2

Embeds the `i18n/` directory into the binary at compile time (`i18n.rs:27-29`). Ensures translations ship inside the binary without needing a separate data directory at runtime.

## Runtime Dependencies (not in Cargo.toml)

### `liquidctl` (system package)

The Python CLI tool that communicates with the AIO over HID. The applet shells out to `liquidctl --match Hydro --json status` every 1500 ms (`app.rs:174`, `liquidctl.rs:109`). Must be installed via `pip` or system package manager.

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
- `vendor` / `vendor-extract` — creates/extracts `vendor.tar` for offline builds
- `tag <version>` — bumps `Cargo.toml` version, commits, and creates a git tag
- `clean` / `clean-vendor` / `clean-dist` — cleanup targets

## Install Paths (justfile)

Binary: `/usr/bin/cosmic-liquid`
Desktop file: `/usr/share/applications/com.github.cosmix.LiquidMon.desktop`
Metainfo: `/usr/share/appdata/com.github.cosmix.LiquidMon.metainfo.xml`
Icon: `/usr/share/icons/hicolor/scalable/apps/com.github.cosmix.LiquidMon.svg`

## Resources

- `resources/app.desktop` — GNOME/freedesktop desktop entry; `X-CosmicApplet=true` marks it as a COSMIC panel applet; `X-CosmicHoverPopup=Auto` enables the hover popup behavior; `NoDisplay=true` hides it from app launchers
- `resources/app.metainfo.xml` — AppStream/AppData metadata for software centers; component id `com.github.cosmix.LiquidMon`
- `resources/icon.svg` — scalable app icon (referenced in justfile install target)

## i18n Stack

- Format: Project Fluent (.ftl files)
- Config: `i18n.toml` — fallback language `en`, assets dir `i18n/`
- Locale files: `i18n/en/cosmic_liquid.ftl` (English strings; currently contains scaffolding strings from the libcosmic applet template, not yet wired to the actual UI)
- Embedded at compile time via `rust-embed` in `i18n.rs`
- System language detected at startup via `DesktopLanguageRequester` in `main.rs`
- Access macro: `fl\!("message-id")` defined in `i18n.rs:44-51`
- Note: The `.ftl` strings (`welcome`, `example-row`, etc.) are boilerplate from the applet template; the actual UI strings in `app.rs` are currently hard-coded Rust string literals, not routed through `fl\!()`

## COSMIC Desktop Registration

The applet is registered with the COSMIC panel via the `.desktop` file:

- `X-CosmicApplet=true` — tells the COSMIC panel this is an applet
- `X-CosmicHoverPopup=Auto` — enables the hover popup mode
- App ID `com.github.cosmix.LiquidMon` must match `APP_ID` in `app.rs:50`
- The COSMIC panel reads installed `.desktop` files from `/usr/share/applications/` to enumerate available applets
