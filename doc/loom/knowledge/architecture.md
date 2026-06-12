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

## Module Dependency Graph — UPDATED 2026-06-12

```text
main.rs  (14 lines)
  ├── mod config    (config.rs)
  ├── mod devices   (devices.rs)
  ├── mod equalizer (equalizer.rs)
  ├── mod liquidctl (liquidctl.rs)
  ├── mod sparkline (sparkline.rs)
  ├── mod spinner   (spinner.rs)
  ├── mod view      (view.rs)      ← NEW module added 2026-06-12
  └── mod app       (app.rs)
        ├── uses crate::config::Config
        ├── uses crate::liquidctl::{AioStatus, DetectedDevice, list_devices, fetch_status}
        ├── uses crate::devices::{filter_aios, auto_select}
        ├── uses crate::sparkline::{Sparkline, SparklineTint}   (panel button only)
        ├── uses crate::spinner::Kind                           (popup spinner glyphs via view)
        └── uses crate::view                                    (popup builders; pub(crate) surface)
```

`main.rs` owns binary entry and delegates entirely to `cosmic::applet::run::<app::AppModel>(())`. The `app` module is the primary consumer of `config`, `liquidctl`, `devices`, and `view`. The `view` module owns the popup widget builders; `equalizer`, `spinner`, and `sparkline` are consumed by `view` (and directly by `app` for the panel button sparkline). The `devices` module depends only on `liquidctl::DetectedDevice`.

**8 source modules total:** `app`, `config`, `devices`, `equalizer`, `liquidctl`, `sparkline`, `spinner`, `view`.

## COSMIC Applet Framework Integration

`AppModel` implements `cosmic::Application` (src/app.rs:70). The framework drives the Iced/Wayland event loop. Key integration points:

- `core: cosmic::Core` (src/app.rs:47) — runtime handle owned by AppModel; passed into AppModel::init by the framework
- `cosmic::applet::run::<AppModel>(())` (src/main.rs:9) — framework entry, replaces a standard Iced `main`
- `AppModel::init` returns `(Self, Task<…>)` — startup task; currently returns `Task::none()`
- `AppModel::subscription` returns `Subscription<Message>` — the framework polls this each render cycle and merges returned subscriptions
- `cosmic::applet::style()` applied via `AppModel::style` (src/app.rs:304-306)
- Popup lifecycle managed through `cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup}` (src/app.rs:7)

Panel button is rendered by `view()` (src/app.rs:120-165); popup overlay by `view_window()` (src/app.rs:170-213). Both are called by the framework on each frame.

## AppModel State Structure — UPDATED 2026-06-12

`src/app.rs:92-132`

```text
AppModel {
    core:                  cosmic::Core           // COSMIC runtime handle
    popup:                 Option<Id>             // Some(id) when popup is open
    config:                Config                 // Persisted config (cosmic_config) — v3
    config_handle:         Option<cosmic_config::Config>  // kept alive for write_entry
    pending_interval_secs: Option<f32>            // slider drag value (None outside drag); cleared on popup close
    last_status:           Option<AioStatus>      // Most-recent successful liquidctl read
    last_error:            Option<String>         // Most-recent error (kept even with stale data)
    temp_history:          VecDeque<f64>          // Liquid temp samples (cap: HISTORY_CAP=900)
    pump_duty_history:     VecDeque<f64>          // Pump duty % samples (popup equalizer)
    fan_avg_duty_history:  VecDeque<f64>          // Mean fan duty % (popup equalizer)
    detected_devices:      Vec<DetectedDevice>    // Latest liquidctl list enumeration
    device_scan_in_flight: bool                   // True while a list_devices Task is in flight
    anim_t:                f32                    // Popup animation clock (seconds); advances only while popup is open
    enumeration_retried:   bool                   // Guards one-shot re-enumeration retry after initial failure
}
```

The popup renders three 64 px VU-meter equalizer canvases (coolant temperature, pump duty, fan-average duty). Pump speed and per-fan speed are surfaced as numeric labels in metric-block headers rather than as separate canvases.

Constants at `src/app.rs:25-33`:

```text
PANEL_SPARK_SAMPLES = 60             — trailing-N window fed to the panel button sparkline
HISTORY_CAP         = 900            — maximum samples in every per-metric VecDeque (~15 min at 1 s)
MIN_INTERVAL_MS     = 1000           — lower bound for the user-configurable sample interval
MAX_INTERVAL_MS     = 10000          — upper bound for the user-configurable sample interval
ANIM_INTERVAL       = Duration(33ms) — drives both the iced `every(...)` subscription and the per-tick anim_t advance
```

On a poll error, `last_status` is intentionally NOT cleared, so the UI can show stale readings alongside the error badge.

## Message/Event Types and Flow — UPDATED 2026-06-12

Defined at `src/app.rs:136-156`:

```text
Message::TogglePopup             — panel button click → open/close popup window
Message::PopupClosed(Id)         — Wayland compositor closed popup externally
Message::UpdateConfig(Config)    — cosmic_config watch fired a new config value
Message::StatusTick { match_str: String, result: Result<AioStatus, String> }
                                 — background subscription delivered a liquidctl result;
                                   carries the match string so late results from old
                                   subscriptions are dropped if the device changed
Message::SampleIntervalDragged(f32)
                                 — fires every drag tick; updates pending_interval_secs only
Message::SampleIntervalReleased  — fires once on slider release; commits and persists the interval
Message::DevicesEnumerated(Result<Vec<DetectedDevice>, String>)
                                 — result of a liquidctl list --json enumeration
Message::DeviceSelected(Option<String>)
                                 — user chose a device from the popup dropdown
Message::AnimationTick           — 33 ms timer; advances anim_t while popup is open
```

All messages route through `AppModel::update` (src/app.rs:372+):

- `StatusTick{Ok}` → appends to `temp_history`/`pump_duty_history`/`fan_avg_duty_history`, writes `last_status`, clears `last_error`
- `StatusTick{Err}` → writes `last_error`, preserves stale `last_status`
- `UpdateConfig` → replaces `self.config`; resets device state if effective match changed
- `TogglePopup` → returns a `Task` (get_popup or destroy_popup), updates `self.popup`; also clears `pending_interval_secs` on close
- `PopupClosed` → clears `self.popup` and `pending_interval_secs` if IDs match
- `DevicesEnumerated(Ok)` → snapshots detected devices; on first failure with no known device fires one bounded delayed retry
- `DeviceSelected` → short-circuits on semantic no-ops; resets device state if effective match changed
- `AnimationTick` → advances `anim_t` by `ANIM_INTERVAL`

## Data Flow: liquidctl subprocess → UI — UPDATED 2026-06-12

```text
Subscription::run_with((interval_ms, match_str), …)    [src/app.rs:319]
  └─ tokio::time::interval + MissedTickBehavior::Delay
       (tick fires at the TOP of the loop so the period equals the configured
        interval regardless of fetch duration; first tick resolves immediately)
       └─ liquidctl::fetch_status(match_str)     [src/liquidctl.rs:132]
            └─ tokio::process::Command::new("liquidctl")
                 .args(["--match", match_str, "--json", "status"])
                 .kill_on_drop(true)
                 .output() — wrapped in tokio::time::timeout(3s)
            └─ parse_status_response(raw)      [src/liquidctl.rs:164]
                 └─ serde_json::from_str → Vec<DeviceEntry>
                      └─ find first device with non-empty status
                      └─ scan StatusEntry vec for known keys
                           "Liquid temperature" → liquid_temp_c: f64
                           "Pump speed"        → pump.speed_rpm: u32
                           "Pump duty"         → pump.duty_pct: u8
                           "Fan N speed/duty"  → fans[N].{speed_rpm,duty_pct}
                      └─ return AioStatus
  └─ channel.send(Message::StatusTick { match_str, result })
       └─ AppModel::update receives StatusTick; drops if match_str ≠ effective_match()
            └─ histories updated; AppModel.last_status updated
                 └─ framework re-renders view() / view_window()
```

Match filter is the description string returned by `AppModel::effective_match()` (config override or auto-pick). Device selection during status parsing still picks the first `DeviceEntry` with a non-empty `status` array (`src/liquidctl.rs`); liquidctl's `--match` performs case-insensitive substring matching against the device description.

## Data Flow: Device Enumeration → Selection

Independent of the status poll loop:

```text
init                               [src/app.rs:163]
  └─ Task::perform(list_devices)
       └─ liquidctl::list_devices()              [src/liquidctl.rs:240]
            └─ tokio::process::Command::new("liquidctl")
                 .args(["list", "--json"])  ── 1 s timeout
            └─ parse_devices_response(raw)        [src/liquidctl.rs:270]
                 └─ Vec<ListDeviceEntry>          [src/liquidctl.rs:286]
                      └─ map → Vec<DetectedDevice>
       └─ Message::DevicesEnumerated(Ok(devs))
            └─ AppModel.detected_devices = devs
            └─ if effective_match() changed → reset_device_state()
            └─ if effective_match() == None → last_error = "no supported AIO detected …"

TogglePopup (open branch)          [src/app.rs:382]
  └─ if !device_scan_in_flight: Task::batch([get_popup, list_devices])
       (re-enumerates so hot-plugged devices appear in the dropdown)

DeviceSelected(Option<String>)     [src/app.rs:425]
  └─ config.device_match = choice
  └─ if effective_match() changed → reset_device_state()
  └─ persist via config.write_entry(handle)
```

`devices::is_aio` consults a substring catalog `AIO_PATTERNS` (`src/devices.rs`) of lowercase patterns covering `hydro_platinum.py`-compatible families (Corsair Hydro Pro/Pro XT/Platinum and iCUE Elite Capellix/RGB in v1). `filter_aios` and `auto_select` are pure functions returning borrowed slices/refs.

All `liquidctl` subprocess calls (`fetch_status`, `list_devices`) acquire a module-private `LIQUIDCTL_LOCK: LazyLock<tokio::sync::Mutex<()>>` (`src/liquidctl.rs:15`) so concurrent invocations cannot race on the exclusive HID claim. Lock is held only for the subprocess duration (≤3 s for status, ≤1 s for list).

## Liquidctl JSON Parsing (src/liquidctl.rs)

Raw JSON schema:

```text
[DeviceEntry]
  bus: String
  address: String
  description: String
  status: [StatusEntry { key: String, value: serde_json::Value, unit: String }]
```

`StatusEntry.value` is typed as `serde_json::Value` (not `serde_json::Number`) so that a string/bool/null value deserializes successfully and the `as_f64()` guard at the loop site silently skips it rather than failing the entire device parse. See "StatusEntry.value type — RESOLVED 2026-06-12" in concerns.md.

Fan parsing: keys matching `"Fan N speed"` / `"Fan N duty"` are accumulated into a `BTreeMap<u8, (Option<u32>, Option<u8>)>` keyed by fan index, then flattened to `Vec<Fan>` sorted by index (`src/liquidctl.rs:158-201`). Index 0 is explicitly rejected by `split_fan_key` (`src/liquidctl.rs:219-221`).

Error hierarchy (`src/liquidctl.rs:52-60`) — UPDATED 2026-06-12:

- `Error::Spawn(io::Error)` — process could not start; `Display` now gives a PATH/install hint when `kind() == NotFound`
- `Error::NonZeroExit { status, stderr }` — liquidctl returned non-zero; `stderr` is truncated to last 4 lines via `last_lines` helper
- `Error::Parse(serde_json::Error)` — JSON malformed
- `Error::NoDevice` — no device matched or no device with non-empty status
- `Error::MissingField { field: &'static str, device: String }` — device found but a required field absent; `field` is `&'static str` (allocation-free matching in tests); `device` is a `String` built only on the cold error path for self-diagnosing messages
- `Error::Timeout` — `tokio::time::timeout(3s)` elapsed (clocks only AFTER `LIQUIDCTL_LOCK` is acquired; lock-wait time does not count)

## Configuration System

`src/config.rs:5-17` — `Config` derives `CosmicConfigEntry` with `#[version = 2]`. It carries one field:

```rust
pub struct Config {
    pub sample_interval_ms: u64,
}
```

`Default` is hand-implemented (not derived) returning `sample_interval_ms: 1500`. The explicit `Default` is required because `#[derive(Default)]` was dropped when the non-default field was added; it also ensures that `CosmicConfigEntry::get_entry`'s field-by-field fallback picks up 1500 ms automatically when upgrading from a v1 config file that has no `sample_interval_ms` key.

Config is loaded in `AppModel::init` (`src/app.rs:161-168`) by constructing a single `cosmic_config::Config` handle, reading the entry from it, and storing both the parsed `Config` and the raw handle in `AppModel` (`config_handle`). Keeping the handle alive is required for `config.write_entry(&handle)` later. Hot-reload via `core().watch_config::<Config>(APP_ID)` subscription remains unchanged (`src/app.rs:293-296`). On load error the framework-provided partial config is used rather than panicking.

APP_ID: `"com.github.cosmix.LiquidMon"` (`src/app.rs:81`)

## View Rendering Logic — UPDATED 2026-06-12

Panel button (`view`, src/app.rs): when `last_status` is present, renders a horizontal `row` of:

- Snowflake + temperature icon (symbolic SVGs, consts now in `view.rs`)
- Temperature text (e.g. `"30.1°"`)
- Sparkline canvas (36×16 px, 60 samples of `temp_history`)
- Fan icon + average fan duty% text
- Pump icon + pump duty% text

On error with no status: `"!"`. Neither: `"…"` (waiting for first reading).

Popup (`view_window`, src/app.rs): three-way match on `(last_status, last_error)`:

- Status + maybe error → calls `self.popup_metrics_view(status, maybe_err.as_deref())`
- No status + error → heading "liquidctl error" + error body
- Neither → "Waiting for first reading…"

`popup_metrics_view` is a thin orchestrator in `app.rs` that reads private `AppModel` fields (`anim_t`, `temp_history`, etc.) and threads plain data into `view::*` builders. The builders themselves are in `src/view.rs` and do not borrow `AppModel`.

## Build and Install

`justfile` — primary build tool. Key targets:

- `just build-release` → `cargo build --release`
- `just run` → `RUST_BACKTRACE=full cargo run --release`
- `just check` → `cargo clippy --all-features -- -W clippy::pedantic`
- `just install` → copies binary to `/usr/bin/liquidmon`, desktop entry, metainfo, and SVG icon

Installed paths use RDNN `com.github.cosmix.LiquidMon`. Vendored dependency workflow available via `just build-vendored`.

## Cross-Cutting Concerns Synthesis

### The "Partially Hardcoded Config" Gap — RESOLVED 2026-05-01

Both runtime behaviors that were previously hardcoded are now config-driven:

- **Sample interval** (resolved 2026-05-01): stored in `Config.sample_interval_ms` (default 1500 ms), user-settable via the slider in the popup, persisted via `config.write_entry`. The subscription re-keys on the clamped interval so the poll loop restarts only on slider release.
- **Device match filter** (resolved 2026-05-01): replaced by automatic enumeration via `liquidctl::list_devices()` plus a user-overridable dropdown. `Config.device_match: Option<String>` (`#[version = 3]`) stores the explicit pick; `None` means "auto-detect". `AppModel::effective_match()` resolves config-or-auto into the description passed to `liquidctl --match`. Subscription re-keys on `(interval_ms, match_str)` so changing either tears down and restarts the poll loop.

### Reliability Chain from Poll to Display

The path `liquidctl subprocess → AioStatus → last_status → view` has one significant remaining failure mode:

1. **Startup race** (`src/app.rs:274`): `main_window_id()` now returns `Option`; the `TogglePopup` arm uses a `let Some(parent) = ... else { self.popup = None; return Task::none(); }` guard — this is resolved.

The stale-data preservation design is intentional and correct — users see recent data with an error badge rather than a blank panel.

### Module Responsibility Summary — UPDATED 2026-06-12

| Module         | Responsibility                                                    | External I/O                          |
| -------------- | ----------------------------------------------------------------- | ------------------------------------- |
| `main.rs`      | Binary entry, framework launch                                    | None                                  |
| `app.rs`       | AppModel, all message handling, `view()`/`view_window()` trait methods, `popup_metrics_view` orchestrator | libcosmic IPC, Wayland popup commands |
| `view.rs`      | Stateless popup widget builders; `pub(crate)` surface; owns `ICON_*` consts, `TEMP_RANGE`/`DUTY_RANGE`, all popup-layer helpers | None |
| `liquidctl.rs` | Subprocess invocation and JSON parsing                            | `liquidctl` process via stdin/stdout  |
| `sparkline.rs` | Iced Canvas widget for panel-button temperature sparkline         | None                                  |
| `equalizer.rs` | Iced Canvas VU-meter widget for popup metric histories            | None                                  |
| `spinner.rs`   | Iced Canvas animated fan/pump glyph widget                        | None                                  |
| `config.rs`    | Config schema declaration only                                    | cosmic-config/dbus (via libcosmic)    |

## CI/CD Pipeline — UPDATED 2026-06-12

`.github/workflows/ci.yml` — present on disk (NOT only in git history; earlier entries claiming otherwise are superseded). Triggered on every push to `main` and every pull request. Concurrency group cancels in-progress PR runs on new pushes (`cancel-in-progress: ${{ github.event_name == 'pull_request' }}`). Runs on `ubuntu-24.04`. Third-party actions pinned to commit SHAs (`actions/checkout`, `Swatinem/rust-cache`); `dtolnay/rust-toolchain@stable` intentionally left as a mutable ref (commented).

**Two jobs:**

Job `check` (fmt / validate / clippy / test / build):

1. Install system deps: `pkg-config`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`, `libfreetype6-dev`, `desktop-file-utils`, `appstream`
2. `desktop-file-validate resources/app.desktop`
3. `appstreamcli validate --no-net resources/app.metainfo.xml`
4. `dtolnay/rust-toolchain@stable` with `rustfmt` and `clippy` components
5. `Swatinem/rust-cache` (SHA-pinned) for dependency caching
6. `cargo fmt --all -- --check`
7. `cargo clippy --all-targets --all-features -- -D warnings` (all warnings are errors via `RUSTFLAGS: -D warnings`)
8. `cargo test --all-features --no-fail-fast`
9. `cargo build --release`

Job `audit` (separate job):

1. `cargo install cargo-audit --locked`
2. `cargo audit`

## Release Workflow — UPDATED 2026-06-12

`.github/workflows/release.yml` — present on disk. Triggered on `v*` tag pushes. Requires `permissions: contents: write`.

Single job `build` (.deb + tarball):

1. Verify Cargo.toml version matches tag (guard: `cargo metadata` vs `GITHUB_REF_NAME`)
2. Same system deps + toolchain + cache as CI (minus desktop-file-utils/appstream)
3. `cargo install cargo-deb --locked`
4. `cargo build --release --locked` then `strip target/release/liquidmon`
5. `cargo deb --no-build --no-strip`
6. Smoke-test: `sudo apt-get install -y ./target/debian/*.deb`, `dpkg -L liquidmon`, `command -v liquidmon`, `sudo apt-get remove -y liquidmon`
7. Tarball: strips `v` from tag → `liquidmon-<version>-x86_64-linux/` with binary + `resources/` + `README.md` (no `justfile` in tarball)
8. `sha256sum *.tar.gz *.deb > SHA256SUMS`
9. `softprops/action-gh-release` (SHA-pinned) uploads `.tar.gz`, `.deb`, `SHA256SUMS`; sets `generate_release_notes: true` and `fail_on_unmatched_files: true`

## cargo-deb Integration (Cargo.toml)

`Cargo.toml` contains `[package.metadata.deb]` (lines 10-26) consumed by `cargo-deb` during releases:

- `maintainer`, `section = "utility"`, `priority = "optional"`
- `depends = "$auto, liquidctl"` — auto-detects Rust runtime deps and adds explicit `liquidctl` dep
- `extended-description` explaining udev rule requirement
- `assets` array maps: binary→`usr/bin/`, desktop→`usr/share/applications/`, metainfo→`usr/share/metainfo/`, icon→`usr/share/icons/hicolor/scalable/apps/`, `README.md`→`usr/share/doc/liquidmon/README`

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

## AppStream Metadata — UPDATED 2026-06-12

`resources/app.metainfo.xml` (AppStream/Flathub standard):

- `metadata_license: CC0-1.0`, `project_license: MPL-2.0`
- `<url type="vcs-browser">https://github.com/cosmix/liquidmon</url>`
- `<requires><display_length compare="ge">360</display_length></requires>` — minimum display width
- `<supports>`: keyboard, pointing, touch controls
- `<content_rating type="oars-1.1" />` — OARS content rating (empty = no objectionable content)
- `<provides><binary>liquidmon</binary></provides>`
- `<releases>` element present with entries for 0.3.0, 0.2.2, 0.2.1, 0.1.4, 0.1.2, 0.1.0
- `<categories>`: System, Monitor
- `<developer id="com.github.cosmix"><name>Dimosthenis Kaponis</name></developer>`

## Icon

`resources/icon.svg` is a stub: a 128×128 empty `<svg>` element with no path data (2 lines). The four symbolic panel icons (`fan-symbolic.svg`, `pump-symbolic.svg`, `snowflake-symbolic.svg`, `temperature-symbolic.svg`) live under `resources/icons/` and are what actually appear in the panel UI.

## Justfile Additional Targets — UPDATED 2026-06-12

Targets and notable changes:

- `clean` — `cargo clean`
- `clean-vendor` — removes `.cargo/` and `vendor/` and `vendor.tar`
- `clean-dist` — runs both `clean` and `clean-vendor`
- `build-debug *args` — `cargo build` (debug profile)
- `check *args` — `cargo clippy --all-features {{args}} -- -W clippy::pedantic` (pedantic warnings, NOT `-D warnings`; non-blocking local lint)
- `check-json` — runs clippy with `--message-format=json` (for editor tooling)
- `ci-local` — mirrors CI exactly: `cargo fmt --check` + `cargo clippy --all-targets --all-features -- -D warnings` + `cargo test` + `cargo build --release`
- `audit` — `cargo audit`
- `hooks` — installs git hooks from `.githooks/install.sh`
- `uninstall` — removes binary, desktop, metainfo, and icon from installed paths (now removes `appdata-dst` too — RESOLVED 2026-06-12)
- `vendor` — POSIX-safe: uses `mktemp` + `sed '$d'` instead of `head -n -1`; archives into `vendor.tar`; removes intermediates (RESOLVED 2026-06-12)
- `vendor-extract` — extracts `vendor.tar` back to `vendor/` and `.cargo/`
- `tag <version>` — has semver guard (`grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'`) before patching; creates annotated tag `v<version>` with message `Release <version>` (RESOLVED 2026-06-12)
- `install` — metainfo installs to `share/metainfo/` (not `share/appdata/`)

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

## Sparkline Fixed Temperature Scale (src/sparkline.rs:41-42) — SUPERSEDED

The sparkline Y-axis was hardcoded to `[10.0, 40.0]` °C and did NOT auto-scale. Any reading outside this band silently mapped to off-canvas coordinates and disappeared. **This entry is historical** — see "Sparkline Auto-Scaled Y-Axis" below for current behavior.

## Sparkline Auto-Scaled Y-Axis (src/sparkline.rs)

The sparkline Y-axis is now computed from the visible sample window via the pure helper `y_range(&[f64]) -> (f64, f64)` (`src/sparkline.rs:35`). Auto-scaling means real spikes and trends fill the canvas vertically; out-of-range readings can no longer disappear off-edge.

A floor `MIN_Y_SPAN: f64 = 2.0` (`src/sparkline.rs:16`) is enforced when the natural sample range is narrower than 2 °C — the band is centered on the data midpoint so flat traces and sub-degree sensor noise render around the canvas midline rather than as amplified false spikes.

Edge-case behavior:

- Empty samples: `y_range` returns `(-1.0, 1.0)` so the caller can compute a midpoint without dividing by zero (no path is drawn for empty input — `draw` early-returns at the `is_empty()` check).
- Single sample: `draw` (`src/sparkline.rs:90-100`) renders a horizontal tick at the sample's y across the full canvas width, so the sparkline is visible immediately after the first poll instead of waiting for a second reading.
- Two or more samples: standard polyline.

## Icon Situation: Stub vs. Symbolic Set — UPDATED 2026-06-12

- `resources/icon.svg` — the application icon installed to `hicolor/scalable/apps/`. Currently an empty 128×128 SVG stub with no paths. The `.deb` and `just install` both deploy it.
- `resources/icons/` — four symbolic SVGs actually used in the panel UI, embedded via `include_bytes!` and now declared as `pub(crate)` consts in `src/view.rs` (moved from `app.rs` during the view-layer extraction):
  - `fan-symbolic.svg` (`view::ICON_FAN`, view.rs:24)
  - `pump-symbolic.svg` (`view::ICON_PUMP`, view.rs:25)
  - `snowflake-symbolic.svg` (`view::ICON_SNOWFLAKE`, view.rs:22-23)
  - `temperature-symbolic.svg` (`view::ICON_TEMP`, view.rs:21)

These symbolic icons are themed/recoloured by the COSMIC compositor (via `symbolic = true` flag in `view::symbolic_icon()`), while the stub app icon means the applet has no distinct launcher icon in software centers.

## CI/CD Architecture (git history: commit 6f9b43b) — SUPERSEDED 2026-06-12

The `.github/workflows/` directory EXISTS ON DISK and is up-to-date. The earlier claim that "workflows exist only in git history / NOT present in the working tree" was stale. See "CI/CD Pipeline — UPDATED 2026-06-12" and "Release Workflow — UPDATED 2026-06-12" above for current detail.

## Release Artifact Set

A tagged release produces three artifacts uploaded to GitHub Releases:

1. `liquidmon_<version>_amd64.deb` — installable Debian package (includes `liquidctl` dep)
2. `liquidmon-<version>-x86_64-linux.tar.gz` — raw tarball (binary + desktop + icon + metainfo)
3. `SHA256SUMS` — checksums for both archives

## Popup Visualization Widgets and Spinner Animation — UPDATED 2026-06-12

The popup metric history is rendered as an 80s graphic-equalizer / VU-meter instead of a smooth sparkline. The panel button still uses `sparkline.rs` (small 36×16 trend glyph); the **popup** uses these two canvas widgets:

- **`equalizer.rs` — `Equalizer { samples, lo, hi }`**: a `canvas::Program` that bins the sample window into a **fixed** number of columns (`(bounds.width / COL_PITCH=7px).max(1)` — count is anchored to width, NOT sample count, so the bar count stays constant as history accumulates). Each column is a stack of `SEGMENTS=10` LED cells lit bottom-up. **Normalisation is against the caller-supplied absolute range `lo..=hi`, clamped — NOT auto-scaled.** This is load-bearing: an auto-scaled window stretches a 0.2 °C wiggle across the whole meter and paints the tallest bar red, making the VU colour ramp meaningless. Colour follows the classic ramp by absolute height (green < `GREEN_MAX=0.6` of stack, amber < `AMBER_MAX=0.85`, red above); topmost lit cell is a full-intensity "peak cap", lower lit cells alpha 0.82, unlit cells theme `background.on` at alpha 0.10. Ranges are defined in **`src/view.rs`** (not `app.rs`): `TEMP_RANGE=(20.0,55.0)` °C for coolant, `DUTY_RANGE=(0.0,100.0)` % for pump/fan duty (the pump/fan meters plot **duty %**, not rpm — rpm is in the header readout only). Sized `Length::Fill`.
- **`spinner.rs` — `Spinner` { kind: `Kind::{Fan,Pump}`, rpm, clock }**: a `canvas::Program` that redraws the same blade/impeller geometry as `fan-symbolic.svg` / `pump-symbolic.svg` (paths transcribed into `Path::bezier_curve_to` calls, scaled by `size/16.0`), rotated by `clock * rpm * RPM_TO_RAD_PER_S` (tuned so ~2000 rpm ≈ one screen turn/sec, not the true ~33/sec). Drawn monochrome in theme `background.on`. Used only in popup metric headers (22×22 px), NOT the panel button.

**Animation loop.** `AppModel` gained `anim_t: f32` (seconds) and `Message::AnimationTick`. `subscription()` appends `cosmic::iced::time::every(33ms).map(|_| AnimationTick)` **only when `self.popup.is_some()`**, so the applet does zero continuous redraw when collapsed. `update` advances `anim_t = (anim_t + 0.033) % 3600.0` (wrap before f32 precision degrades; spinners read it modulo a full turn). Each `Spinner` receives `self.anim_t` and computes its own angle from its rpm — one shared clock drives differently-paced fan and pump glyphs.

## Popup Layout (redesigned — src/app.rs::popup_metrics_view)

The popup is a `scrollable` `Column` (spacing 14, padding 16) of: device title (`heading` size 16) → divider → three `metric_block`s → per-fan breakdown (`fan_rows`, indented 8px) → divider → `interval_control` → `device_dropdown_section` → optional error caption. A `metric_block(glyph, label, value, history)` is a header `row![glyph, caption(LABEL), Space::new().width(Fill), mono value]` over the `Equalizer` canvas (`eq_canvas`, 64px tall). Labels are small-caps captions ("COOLANT"/"PUMP"/"FANS"/"SAMPLE INTERVAL"/"DEVICE"); numeric readouts are right-aligned mono via `metric_value(text, size)` (coolant 20px, pump/fan 15px). The coolant header carries a static 18px snowflake glyph; pump/fan headers carry animated `Spinner` glyphs. `Space::new().width(...)` is this iced fork's filler idiom — `Space::new()` takes no args and is configured via builder methods (`Space::with_width` does NOT exist here).
