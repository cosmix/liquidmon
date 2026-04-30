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
