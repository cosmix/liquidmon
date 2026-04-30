# Architecture

> High-level component relationships, data flow, and module dependencies.
> This file is append-only - agents add discoveries, never delete.

## Entry Points

- `src/main.rs` — Rust CLI entry point

## Directory Structure

```text
Cargo.lock
Cargo.toml
README.md
doc/
  loom/
    knowledge/
justfile
resources/
scripts/
src/
```

## Module Dependency Graph

```text
main.rs
  ├── mod config    (config.rs)
  ├── mod liquidctl (liquidctl.rs)
  ├── mod sparkline (sparkline.rs)
  └── mod app       (app.rs)
        ├── uses crate::config::Config
        ├── uses crate::liquidctl::AioStatus
        └── uses crate::sparkline::Sparkline
```

`main.rs` owns binary entry and delegates entirely to `cosmic::applet::run::<app::AppModel>(())`. The `app` module is the only consumer of `config`, `liquidctl`, and `sparkline`.

## COSMIC Applet Framework Integration

`AppModel` implements `cosmic::Application` (src/app.rs:70). The framework drives the Iced/Wayland event loop. Key integration points:

- `core: cosmic::Core` (src/app.rs:47) — runtime handle owned by AppModel; passed into AppModel::init by the framework
- `cosmic::applet::run::<AppModel>(())` (src/main.rs:9) — framework entry, replaces a standard Iced `main`
- `AppModel::init` returns `(Self, Task<…>)` — startup task; currently returns `Task::none()`
- `AppModel::subscription` returns `Subscription<Message>` — the framework polls this each render cycle and merges returned subscriptions
- `cosmic::applet::style()` applied via `AppModel::style` (src/app.rs:304-306)
- Popup lifecycle managed through `cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup}` (src/app.rs:7)

Panel button is rendered by `view()` (src/app.rs:120-165); popup overlay by `view_window()` (src/app.rs:170-213). Both are called by the framework on each frame.

## AppModel State Structure

`src/app.rs:44-58`

```text
AppModel {
    core:         cosmic::Core           // COSMIC runtime handle
    popup:        Option<Id>             // Some(id) when popup is open
    config:       Config                 // Persisted config (cosmic_config)
    last_status:  Option<AioStatus>      // Most-recent successful liquidctl read
    last_error:   Option<String>         // Most-recent error (kept even with stale data)
    temp_history: VecDeque<f64>          // Liquid temp samples for panel sparkline (cap: MAX_SAMPLES=60)
}
```

`MAX_SAMPLES = 60` at `src/app.rs:22`. At 1500 ms per sample, this covers ~90 s of history. On a poll error, `last_status` is intentionally NOT cleared (src/app.rs:268), so the UI can show stale readings alongside the error badge.

## Message/Event Types and Flow

Defined at `src/app.rs:61-67`:

```text
Message::TogglePopup             — panel button click → open/close popup window
Message::PopupClosed(Id)         — Wayland compositor closed popup externally
Message::UpdateConfig(Config)    — cosmic_config watch fired a new config value
Message::StatusTick(Result<AioStatus, String>)
                                 — background subscription delivered a liquidctl result
```

All messages route through `AppModel::update` (src/app.rs:253-302):

- `StatusTick(Ok)` → appends temp to `temp_history`, writes `last_status`, clears `last_error`
- `StatusTick(Err)` → writes `last_error`, preserves stale `last_status`
- `UpdateConfig` → replaces `self.config`
- `TogglePopup` → returns a `Task` (get_popup or destroy_popup), updates `self.popup`
- `PopupClosed` → clears `self.popup` if IDs match

## Data Flow: liquidctl subprocess → UI

```text
Subscription::run_with("liquidctl-sub", …)    [src/app.rs:224]
  └─ infinite async loop every 1500 ms
       └─ liquidctl::fetch_status("Hydro")     [src/liquidctl.rs:115]
            └─ tokio::process::Command::new("liquidctl")
                 .args(["--match", "Hydro", "--json", "status"])
                 .kill_on_drop(true)
                 .output() — wrapped in tokio::time::timeout(3s)
            └─ parse_status_response(raw)      [src/liquidctl.rs:142]
                 └─ serde_json::from_str → Vec<DeviceEntry>
                      └─ find first device with non-empty status
                      └─ scan StatusEntry vec for known keys
                           "Liquid temperature" → liquid_temp_c: f64
                           "Pump speed"        → pump.speed_rpm: u32
                           "Pump duty"         → pump.duty_pct: u8
                           "Fan N speed/duty"  → fans[N].{speed_rpm,duty_pct}
                      └─ return AioStatus
  └─ channel.send(Message::StatusTick(result))
       └─ AppModel::update receives StatusTick
            └─ temp_history updated; AppModel.last_status updated
                 └─ framework re-renders view() / view_window()
```

Match filter is hardcoded to `"Hydro"` at `src/app.rs:229`. Device selection picks the first `DeviceEntry` with a non-empty `status` array (`src/liquidctl.rs:145-148`).

## Liquidctl JSON Parsing (src/liquidctl.rs)

Raw JSON schema:

```text
[DeviceEntry]
  bus: String
  address: String
  description: String
  status: [StatusEntry { key: String, value: Number, unit: String }]
```

Fan parsing: keys matching `"Fan N speed"` / `"Fan N duty"` are accumulated into a `BTreeMap<u8, (Option<u32>, Option<u8>)>` keyed by fan index, then flattened to `Vec<Fan>` sorted by index (`src/liquidctl.rs:158-201`). Index 0 is explicitly rejected by `split_fan_key` (`src/liquidctl.rs:219-221`).

Error hierarchy (`src/liquidctl.rs:33-44`):

- `Error::Spawn(io::Error)` — process could not start
- `Error::NonZeroExit { status, stderr }` — liquidctl returned non-zero
- `Error::Parse(serde_json::Error)` — JSON malformed
- `Error::NoDevice` — no device matched or no device with non-empty status
- `Error::MissingField(&'static str)` — device found but a required field (liquid temperature, pump speed, pump duty) is absent
- `Error::Timeout` — `tokio::time::timeout(3s)` elapsed before the subprocess completed

## Configuration System

`src/config.rs:5-7` — `Config` derives `CosmicConfigEntry` with `#[version = 1]`. The struct body is empty (`pub struct Config {}`); the `CosmicConfigEntry` derive accepts an empty struct. Config is loaded in `AppModel::init` via `cosmic_config::Config::new(APP_ID, Config::VERSION)` and hot-reloaded via `core().watch_config::<Config>(APP_ID)` subscription (`src/app.rs:241-244`). On load error the framework-provided partial config is used rather than panicking.

APP_ID: `"com.github.cosmix.LiquidMon"` (`src/app.rs:81`)

## View Rendering Logic

Panel button (`view`, src/app.rs:120-165): when `last_status` is present, renders a horizontal `row` of:

- Snowflake + temperature icon (symbolic SVGs)
- Temperature text (e.g. `"30.1°"`)
- Sparkline canvas (36×16 px, 60 samples of `temp_history`)
- Fan icon + average fan duty% text
- Pump icon + pump duty% text

On error with no status: `"!"`. Neither: `"…"` (waiting for first reading).

Popup (`view_window`, src/app.rs:170-213): three-way match on `(last_status, last_error)`:

- Status + maybe error → `list_column` with heading (device description), liquid temp body, pump body, one body per fan, optional error caption
- No status + error → heading "liquidctl error" + error body
- Neither → "Waiting for first reading…"

## Build and Install

`justfile` — primary build tool. Key targets:

- `just build-release` → `cargo build --release`
- `just run` → `RUST_BACKTRACE=full cargo run --release`
- `just check` → `cargo clippy --all-features -- -W clippy::pedantic`
- `just install` → copies binary to `/usr/bin/liquidmon`, desktop entry, metainfo, and SVG icon

Installed paths use RDNN `com.github.cosmix.LiquidMon`. Vendored dependency workflow available via `just build-vendored`.

## Cross-Cutting Concerns Synthesis

### The "No Real Config" Gap

The configuration infrastructure (`src/config.rs`, `src/app.rs:97-108`, `src/app.rs:241-244`) is fully wired for hot-reload, but the `Config` struct is empty. Two critical runtime behaviors are therefore hardcoded outside the config system:

- Device match filter: `"Hydro"` at `src/app.rs:229`
- Poll interval: `1500 ms` at `src/app.rs:235`

When these are moved to `Config`, the subscription closure in `src/app.rs:222-246` must receive them as captured values.

### Reliability Chain from Poll to Display

The path `liquidctl subprocess → AioStatus → last_status → view` has one significant remaining failure mode:

1. **Startup race** (`src/app.rs:274`): `main_window_id()` now returns `Option`; the `TogglePopup` arm uses a `let Some(parent) = ... else { self.popup = None; return Task::none(); }` guard — this is resolved.

The stale-data preservation design is intentional and correct — users see recent data with an error badge rather than a blank panel.

### Module Responsibility Summary

| Module         | Responsibility                                     | External I/O                          |
| -------------- | -------------------------------------------------- | ------------------------------------- |
| `main.rs`      | Binary entry, framework launch                     | None                                  |
| `app.rs`       | AppModel, all message handling, all view rendering | libcosmic IPC, Wayland popup commands |
| `liquidctl.rs` | Subprocess invocation and JSON parsing             | `liquidctl` process via stdin/stdout  |
| `sparkline.rs` | Iced Canvas widget for temperature sparkline       | None                                  |
| `config.rs`    | Config schema declaration only                     | cosmic-config/dbus (via libcosmic)    |

## CI/CD Pipeline

`.github/workflows/ci.yml` — triggered on every push to `main` and every pull request. Concurrency group cancels in-progress PR runs on new pushes (`cancel-in-progress: ${{ github.event_name == 'pull_request' }}`). Runs on `ubuntu-24.04`.

Single job `check` (fmt / clippy / test / build):

1. Install system deps: `pkg-config`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`, `libfreetype6-dev`
2. `dtolnay/rust-toolchain@stable` with `rustfmt` and `clippy` components
3. `Swatinem/rust-cache@v2` for dependency caching
4. `cargo fmt --all -- --check`
5. `cargo clippy --all-targets --all-features -- -D warnings` (all warnings are errors via `RUSTFLAGS: -D warnings`)
6. `cargo test --all-features --no-fail-fast`
7. `cargo build --release`

## Release Workflow

`.github/workflows/release.yml` — triggered on `v*` tag pushes. Requires `permissions: contents: write` for GitHub release creation.

Single job `build` (.deb + tarball):

1. Same system deps + toolchain + cache as CI
2. `cargo install cargo-deb --locked` — installs the Debian packager
3. `cargo build --release --locked` then `strip target/release/liquidmon`
4. `cargo deb --no-build --no-strip` — generates `.deb` from `[package.metadata.deb]` in `Cargo.toml`
5. Smoke-test: `sudo dpkg -i target/debian/*.deb`, `dpkg -L liquidmon`, `command -v liquidmon`, `sudo dpkg -r liquidmon`
6. Tarball: strips `v` from tag → `liquidmon-<version>-x86_64-linux/` with binary + `resources/` + `justfile` + `README.md`, then `tar -czf`
7. `sha256sum *.tar.gz *.deb > SHA256SUMS`
8. `softprops/action-gh-release@v2` uploads `.tar.gz`, `.deb`, `SHA256SUMS`; sets `generate_release_notes: true`

## cargo-deb Integration (Cargo.toml)

`Cargo.toml` contains `[package.metadata.deb]` (lines 10-26) consumed by `cargo-deb` during releases:

- `maintainer`, `section = "utility"`, `priority = "optional"`
- `depends = "$auto, liquidctl"` — auto-detects Rust runtime deps and adds explicit `liquidctl` dep
- `extended-description` explaining udev rule requirement
- `assets` array maps: binary→`usr/bin/`, desktop→`usr/share/applications/`, metainfo→`usr/share/appdata/`, icon→`usr/share/icons/hicolor/scalable/apps/`, `README.md`→`usr/share/doc/liquidmon/README`

## libcosmic Dependency Pinning

`libcosmic` is sourced directly from git (`pop-os/libcosmic`) pinned to commit `564ef834cec33a948dc10c9b401cf29db5d18373` (`Cargo.toml:35-37`). Features enabled: `applet`, `applet-token`, `dbus-config`, `multi-window`, `tokio`, `wayland`, `winit`. No registry version is used — upstream does not publish to crates.io.

## Cargo Edition and No Profiles

`Cargo.toml` uses `edition = "2024"` (Rust 2024 edition). There are no custom `[profile.*]` sections; release builds use Cargo defaults.

## Desktop Entry Fields

`resources/app.desktop` notable fields beyond name/icon/exec:

- `NoDisplay=true` — hides applet from standard application launchers
- `X-CosmicApplet=true` — COSMIC-specific key marking it as a panel applet
- `X-CosmicHoverPopup=Auto` — controls hover popup behavior in the COSMIC panel
- `StartupNotify=true`, `Terminal=false`, `Categories=COSMIC`, `MimeType=` (explicitly empty)

## AppStream Metadata

`resources/app.metainfo.xml` (AppStream/Flathub standard):

- `metadata_license: CC0-1.0`, `project_license: MPL-2.0`
- `<url type="vcs-browser">https://github.com/cosmix/liquidmon</url>`
- `<requires><display_length compare="ge">360</display_length></requires>` — minimum display width
- `<supports>`: keyboard, pointing, touch controls
- `<content_rating type="oars-1.1" />` — OARS content rating (empty = no objectionable content)
- `<provides><binaries><binary>liquidmon</binary></binaries></provides>`

## Icon

`resources/icon.svg` is a stub: a 128×128 empty `<svg>` element with no path data (2 lines). The four symbolic panel icons (`fan-symbolic.svg`, `pump-symbolic.svg`, `snowflake-symbolic.svg`, `temperature-symbolic.svg`) live under `resources/icons/` and are what actually appear in the panel UI.

## Justfile Additional Targets

Targets not previously documented:

- `clean` — `cargo clean`
- `clean-vendor` — removes `.cargo/` and `vendor/` and `vendor.tar`
- `clean-dist` — runs both `clean` and `clean-vendor`
- `build-debug *args` — `cargo build` (debug profile)
- `check-json` — runs clippy with `--message-format=json` (for editor tooling)
- `uninstall` — removes binary, desktop, and icon from installed paths (does NOT remove metainfo)
- `vendor` — runs `cargo vendor`, pipes source replacement config into `.cargo/config.toml`, archives everything into `vendor.tar`, then removes `.cargo/` and `vendor/` (the archive is the artifact)
- `vendor-extract` — extracts `vendor.tar` back to `vendor/` and `.cargo/`
- `tag <version>` — sed-patches all `Cargo.toml` `version =` lines, runs `cargo check` + `cargo clean`, stages `Cargo.lock`, creates a `release: <version>` commit, and creates an annotated git tag

## doc/plans Directory

`doc/plans/` exists but is currently empty — no active plans.

## .cargo Directory

No `.cargo/config.toml` is checked into the repo. It is generated transiently by `just vendor` and included inside `vendor.tar`; it is absent from the working tree when not using vendored builds.

## Debian Packaging via cargo-deb (Cargo.toml:10-26)

`[package.metadata.deb]` wires up `cargo-deb` as the authoritative packaging source. Key fields:

- `depends = "$auto, liquidctl"` — `$auto` resolves shared-library deps at package time; `liquidctl` is declared as an explicit runtime dep, so `.deb` consumers get it automatically
- `assets` list mirrors the `just install` paths exactly; if either diverges, the install is inconsistent
- `extended-description` re-states the udev/liquidctl prerequisite for software-center consumers
- `priority = "optional"`, `section = "utility"` — standard Debian classification

The release workflow uses `cargo install cargo-deb` then `cargo deb --no-build` (since the binary is already stripped) to produce the `.deb` artifact.

## Sparkline Fixed Temperature Scale (src/sparkline.rs:41-42)

The sparkline Y-axis is hardcoded to `[10.0, 40.0]` °C — it does NOT auto-scale to data. Any reading above 40 °C pins at the top edge without visual distinction. This is a soft limit; real AIO liquid temps rarely exceed 40 °C under normal load, but the limitation is invisible to the user and creates false-flat sparklines during thermal events.

## Icon Situation: Stub vs. Symbolic Set

- `resources/icon.svg` — the application icon installed to `hicolor/scalable/apps/`. Currently an empty 128×128 SVG stub with no paths. The `.deb` and `just install` both deploy it.
- `resources/icons/` — four symbolic SVGs actually used in the panel UI, embedded via `include_bytes\!`:
  - `fan-symbolic.svg` (ICON_FAN, app.rs:25)
  - `pump-symbolic.svg` (ICON_PUMP, app.rs:26)
  - `snowflake-symbolic.svg` (ICON_SNOWFLAKE, app.rs:24)
  - `temperature-symbolic.svg` (ICON_TEMP, app.rs:23)

These symbolic icons are themed/recoloured by the COSMIC compositor (via `symbolic = true` flag in `symbolic_icon()`), while the stub app icon means the applet has no distinct launcher icon in software centers.

## CI/CD Architecture (git history: commit 6f9b43b)

The `.github/workflows/` directory exists in git history but is NOT present in the working tree. The workflows are:

### ci.yml

- Trigger: push to `main`, PRs (with concurrency cancel-in-progress)
- Runner: `ubuntu-24.04`
- Toolchain: `dtolnay/rust-toolchain@stable` (floating, not pinned to a version)
- Cache: `Swatinem/rust-cache@v2`
- Steps in order: `cargo fmt --check` → `cargo clippy` (with `RUSTFLAGS=-D warnings`) → `cargo test` → `cargo build --release`

### release.yml

- Trigger: tags matching `v*`
- Steps: install `cargo-deb` → `cargo build --release` → strip binary → `cargo deb --no-build` → smoke-test install/uninstall → create `liquidmon-<version>-x86_64-linux.tar.gz` + `SHA256SUMS` → upload all artifacts via `softprops/action-gh-release@v2`

## Release Artifact Set

A tagged release produces three artifacts uploaded to GitHub Releases:

1. `liquidmon_<version>_amd64.deb` — installable Debian package (includes `liquidctl` dep)
2. `liquidmon-<version>-x86_64-linux.tar.gz` — raw tarball (binary + desktop + icon + metainfo)
3. `SHA256SUMS` — checksums for both archives
