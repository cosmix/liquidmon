# Stack & Dependencies

> Project technology stack, frameworks, and key dependencies.
> This file is append-only - agents add discoveries, never delete.

## Project Type

- **Rust** (Cargo.toml found)

## Rust Toolchain and Edition

- Rust edition 2024 (`Cargo.toml:4`)
- Package: `liquidmon` v0.1.0, license MPL-2.0

## Direct Dependencies

### `libcosmic` (git = "<https://github.com/pop-os/libcosmic.git>", rev = "564ef834cec33a948dc10c9b401cf29db5d18373")

The core UI and application framework from System76/Pop!\_OS. Provides the `cosmic::Application` trait, widget toolkit (iced-based), applet runtime, config system, and theming. Pinned to a specific rev for reproducibility.

**Features enabled:**

- `applet` — Activates the panel-applet mode of the COSMIC app framework; required for `cosmic::applet::run` and `core.applet.*` helpers
- `applet-token` — Provides the applet token mechanism used by the COSMIC compositor to identify and position panel applets
- `dbus-config` — Hooks into `cosmic-settings-daemon` to watch for config file changes; enables `core.watch_config::<Config>()` subscription
- `multi-window` — Enables the popup/secondary-window system used for the hover popup
- `tokio` — Uses Tokio as the async executor for the COSMIC runtime; required because the app spawns async tasks (liquidctl polling)
- `wayland` — Adds Wayland window/surface support via winit; required on Pop!\_OS/COSMIC which is Wayland-only
- `winit` — Windowing backend; provides the event loop and window management layer

### `tokio` v1.x, features = ["process", "time"]

Async runtime. Used for `tokio::process::Command` to spawn `liquidctl` as a subprocess (`liquidctl.rs:9,116`) and `tokio::time::sleep` / `tokio::time::timeout` for the configurable polling interval (default 1500 ms, `app.rs:286`) and 3 s subprocess timeout (`liquidctl.rs:119`). Features narrowed from `["full"]` to only the two features actually used.

### `serde` v1.0.228, features = ["derive"]

Serialization framework. Used to derive `Deserialize` on `DeviceEntry` and `StatusEntry` (`liquidctl.rs:95,105`) to parse liquidctl JSON output.

### `serde_json` v1.0.149

JSON parsing. Used in `liquidctl.rs:143` to deserialize the raw liquidctl `--json status` output into `Vec<DeviceEntry>`. Also the type of `StatusEntry::value` (`serde_json::Number`, `liquidctl.rs:108`).

### `futures-util` v0.3.31

Async stream utilities. Provides `SinkExt` (imported at `app.rs:14`) for `.send()` on the mpsc channel inside the liquidctl subscription stream, and `futures_util::future::pending()` for the infinite-loop sentinel (`app.rs:237`).

## Runtime Dependencies (not in Cargo.toml)

### `liquidctl` (system package)

The Python CLI tool that communicates with the AIO over HID. The applet shells out to `liquidctl --match Hydro --json status` every `config.sample_interval_ms` (default 1500 ms, `app.rs:286`, `liquidctl.rs:116`). Must be installed via `pip` or system package manager.

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

## CI Workflow (.github/workflows/ci.yml)

Defined in commit 6f9b43b but NOT currently present on disk (workflows were committed then the directory was removed/untracked — the file exists only in git history at `git show 6f9b43b:.github/workflows/ci.yml`).

**Triggers:** push to `main`, all pull_requests (with concurrency cancel-in-progress for PRs)

**Runner:** ubuntu-24.04

**Rust toolchain:** `dtolnay/rust-toolchain@stable` — no pinned version; always uses latest stable. Components: `rustfmt`, `clippy`.

**Caching:** `Swatinem/rust-cache@v2`

**System deps installed:** `pkg-config`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`, `libfreetype6-dev`

**RUSTFLAGS:** `-D warnings` (global env, makes all warnings errors)

**Steps (single job `check`):**

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --all-features --no-fail-fast`
4. `cargo build --release`

## Release Workflow (.github/workflows/release.yml)

Also in git history only (same commit 6f9b43b). Trigger: push of tags matching `v*`.

**Permissions:** `contents: write` (to create GitHub releases)

**Artifacts produced:**

- Stripped release binary via `cargo build --release --locked`
- `.deb` package via `cargo-deb` (`cargo install cargo-deb --locked`, then `cargo deb --no-build --no-strip`)
- Source tarball: `liquidmon-<version>-x86_64-linux.tar.gz` containing binary + `resources/` + `justfile` + `README.md`
- `SHA256SUMS` file covering both `.tar.gz` and `.deb`

**Tarball naming:** `liquidmon-${version}-x86_64-linux.tar.gz` where version strips the `v` prefix from the git tag

**Upload:** `softprops/action-gh-release@v2` with `generate_release_notes: true`

**Deb verification step:** installs with `dpkg -i`, checks `dpkg -L`, verifies binary in PATH, then removes — ensures the package is installable before release.

## Cargo.toml — Complete Structure

The Cargo.toml has NO `[profile.*]`, `[features]`, `[[bin]]`, or `[package.metadata]` sections. It contains only `[package]` and `[dependencies]` / `[dependencies.libcosmic]`. The binary name defaults to the package name `liquidmon`. Tokio version spec in Cargo.toml is `"1.48.0"` (minimum), resolving to `1.52.1` in Cargo.lock.

## Key Resolved Dependency Versions (from Cargo.lock)

| Crate                      | Resolved Version | Source                                          |
| -------------------------- | ---------------- | ----------------------------------------------- |
| `libcosmic`                | 1.0.0            | git rev `564ef834` (pop-os/libcosmic)           |
| `iced`                     | 0.14.0           | git rev `564ef834` (pop-os/libcosmic fork)      |
| `iced_core`                | 0.14.0           | git rev `564ef834` (pop-os/libcosmic fork)      |
| `iced_futures`             | 0.14.0           | git rev `564ef834` (pop-os/libcosmic fork)      |
| `tokio`                    | 1.52.1           | crates.io                                       |
| `serde`                    | 1.0.228          | crates.io                                       |
| `serde_json`               | 1.0.149          | crates.io                                       |
| `futures-util`             | 0.3.32           | crates.io (spec was 0.3.31, resolved to 0.3.32) |
| `cosmic-config`            | 1.0.0            | git rev `564ef834` (pop-os/libcosmic fork)      |
| `cosmic-text`              | present          | pop-os/libcosmic fork                           |
| `cosmic-panel-config`      | 0.1.0            | pop-os/cosmic-panel                             |
| `cosmic-protocols`         | 0.2.0            | pop-os/cosmic-protocols rev `160b086`           |
| `cosmic-freedesktop-icons` | 0.4.0            | pop-os/freedesktop-icons                        |
| `cosmic-settings-daemon`   | 0.1.0            | pop-os/dbus-settings-bindings                   |

All iced sub-crates (`iced`, `iced_core`, `iced_futures`, `iced_runtime`, `iced_widget`) are sourced from the pop-os/libcosmic git repo (not upstream iced), pinned to the same commit as libcosmic.

## Toolchain Files

No `rust-toolchain.toml`, `.rustfmt.toml`, or `clippy.toml` exist in the repository. The CI pins to `stable` via the `dtolnay/rust-toolchain@stable` action. Local builds use whatever rustup default is active.

## Resources — Icon Files

`resources/icons/` contains four symbolic SVG icons used by the applet UI:

- `fan-symbolic.svg` — fan speed indicator
- `pump-symbolic.svg` — pump duty indicator
- `snowflake-symbolic.svg` — cooling/liquid indicator
- `temperature-symbolic.svg` — temperature indicator

`resources/icon.svg` — the main app icon installed to `hicolor/scalable/apps/com.github.cosmix.LiquidMon.svg`

## Justfile — Complete Target List

All targets (none were undocumented beyond what was known):

- `default` → alias for `build-release`
- `clean` — `cargo clean`
- `clean-vendor` — removes `.cargo/` and `vendor/` and `vendor.tar`
- `clean-dist` — `clean` + `clean-vendor`
- `build-debug *args` — `cargo build {{args}}`
- `build-release *args` — calls `build-debug '--release' args`
- `build-vendored *args` — `vendor-extract` + `build-release '--frozen --offline' args`
- `check *args` — `cargo clippy --all-features` with `-W clippy::pedantic`
- `check-json` — `check '--message-format=json'`
- `run *args` — `RUST_BACKTRACE=full cargo run --release`
- `install` — installs binary (0755), desktop, metainfo (0644), icon (0644)
- `uninstall` — removes binary, desktop, icon
- `vendor` — vendors deps into `vendor.tar` + `.cargo/config.toml` patch
- `vendor-extract` — unpacks `vendor.tar`
- `tag <version>` — bumps version in all Cargo.toml files via `sed`, does `cargo check` + `cargo clean`, commits with `release: <version>`, creates annotated git tag
