# Architecture

> High-level component relationships, data flow, and module dependencies.
> This file is append-only - agents add discoveries, never delete.

(Add architecture diagrams and component relationships as you discover them)

## Entry Points

- `src/main.rs` - Rust CLI entry point

## Directory Structure

```
Cargo.lock
Cargo.toml
README.md
doc/
  loom/
    knowledge/
i18n/
  en/
i18n.toml
justfile
resources/
scripts/
src/
```

## Module Dependency Graph

```
main.rs
  ├── mod i18n      (i18n.rs)
  ├── mod config    (config.rs)
  ├── mod liquidctl (liquidctl.rs)
  └── mod app       (app.rs)
        ├── uses crate::config::Config
        └── uses crate::liquidctl::AioStatus
```

`main.rs` owns binary entry: initialises i18n, then delegates entirely to `cosmic::applet::run::<app::AppModel>(())`. The `app` module is the only consumer of `config` and `liquidctl`; `i18n` is standalone after init.

## COSMIC Applet Framework Integration

`AppModel` implements `cosmic::Application` (src/app.rs:39). The framework drives the Iced/Wayland event loop. Key integration points:

- `core: cosmic::Core` (src/app.rs:18) — runtime handle owned by AppModel; passed into AppModel::init by the framework
- `cosmic::applet::run::<AppModel>(())` (src/main.rs:16) — framework entry, replaces a standard Iced `main`
- `AppModel::init` returns `(Self, Task<…>)` — startup task; currently returns `Task::none()`
- `AppModel::subscription` returns `Subscription<Message>` — the framework polls this each render cycle and merges returned subscriptions
- `cosmic::applet::style()` applied via `AppModel::style` (src/app.rs:247-249)
- Popup lifecycle managed through `cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup}` (src/app.rs:5)

Panel button is rendered by `view()` (src/app.rs:95-110); popup overlay by `view_window()` (src/app.rs:115-158). Both are called by the framework on each frame.

## AppModel State Structure

`src/app.rs:16-27`

```
AppModel {
    core:        cosmic::Core           // COSMIC runtime handle
    popup:       Option<Id>             // Some(id) when popup is open
    config:      Config                 // Persisted config (cosmic_config)
    last_status: Option<AioStatus>      // Most-recent successful liquidctl read
    last_error:  Option<String>         // Most-recent error (kept even with stale data)
}
```

Note: on a poll error, `last_status` is intentionally NOT cleared (src/app.rs:214-215), so the UI can show stale readings alongside the error badge.

## Message/Event Types and Flow

Defined at `src/app.rs:31-36`:

```
Message::TogglePopup             — panel button click → open/close popup window
Message::PopupClosed(Id)         — Wayland compositor closed popup externally
Message::UpdateConfig(Config)    — cosmic_config watch fired a new config value
Message::StatusTick(Result<AioStatus, String>)
                                 — background subscription delivered a liquidctl result
```

All messages route through `AppModel::update` (src/app.rs:204-245):

- `StatusTick(Ok)` → writes `last_status`, clears `last_error`
- `StatusTick(Err)` → writes `last_error`, preserves stale `last_status`
- `UpdateConfig` → replaces `self.config`
- `TogglePopup` → returns a `Task` (get_popup or destroy_popup), updates `self.popup`
- `PopupClosed` → clears `self.popup` if IDs match

## Data Flow: liquidctl subprocess → UI

```
Subscription::run_with("liquidctl-sub", …)    [src/app.rs:169]
  └─ infinite async loop every 1500 ms
       └─ liquidctl::fetch_status("Hydro")     [src/liquidctl.rs:108]
            └─ tokio::process::Command::new("liquidctl")
                 .args(["--match", "Hydro", "--json", "status"])
                 .output().await
            └─ parse_status_response(raw)      [src/liquidctl.rs:133]
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
            └─ AppModel.last_status updated
                 └─ framework re-renders view() / view_window()
```

Match filter is hardcoded to `"Hydro"` at `src/app.rs:174`. Device selection picks the first `DeviceEntry` with a non-empty `status` array (`src/liquidctl.rs:136-139`).

## Liquidctl JSON Parsing (src/liquidctl.rs)

Raw JSON schema:

```
[DeviceEntry]
  bus: String
  address: String
  description: String
  status: [StatusEntry { key: String, value: Number, unit: String }]
```

Fan parsing: keys matching `"Fan N speed"` / `"Fan N duty"` are accumulated into a `BTreeMap<u8, (Option<u32>, Option<u8>)>` keyed by fan index, then flattened to `Vec<Fan>` sorted by index (`src/liquidctl.rs:144-188`). Index 0 is explicitly rejected by `split_fan_key` (`src/liquidctl.rs:207-209`).

Error hierarchy (`src/liquidctl.rs:33-41`):

- `Error::Spawn(io::Error)` — process could not start
- `Error::NonZeroExit { status, stderr }` — liquidctl returned non-zero
- `Error::Parse(serde_json::Error)` — JSON malformed
- `Error::NoDevice` — no device matched or required keys missing

## Configuration System

`src/config.rs:5-9` — `Config` derives `CosmicConfigEntry` with `#[version = 1]`. Currently contains only a placeholder `demo: String` field. Config is loaded in `AppModel::init` via `cosmic_config::Config::new(APP_ID, Config::VERSION)` and hot-reloaded via `core().watch_config::<Config>(APP_ID)` subscription (`src/app.rs:187-195`). On load error the framework-provided partial config is used rather than panicking.

APP_ID: `"com.github.cosmix.LiquidMon"` (`src/app.rs:50`)

## View Rendering Logic

Panel button (`view`, src/app.rs:95-110): renders one text widget:

- `last_status` present → `"NN.N°"` (liquid temp, one decimal)
- `last_error` present, no status → `"\!"`
- Neither → `"…"` (waiting for first reading)

Popup (`view_window`, src/app.rs:115-158): three-way match on `(last_status, last_error)`:

- Status + maybe error → `list_column` with heading (device description), liquid temp body, pump body, one body per fan, optional error caption
- No status + error → heading "liquidctl error" + error body
- Neither → "Waiting for first reading…"

## Internationalisation

`src/i18n.rs` uses `i18n_embed` + `rust_embed`. Translation files are compiled into the binary via `RustEmbed` on the `i18n/` directory. `fl\!()` macro (`src/i18n.rs:44-52`) resolves IDs at runtime from `LANGUAGE_LOADER` (a `LazyLock<FluentLanguageLoader>`). Languages are selected from `DesktopLanguageRequester` in `main.rs:10`. The `fl\!()` macro is currently unused in `app.rs`; all visible strings are plain Rust literals.

## Build and Install

`justfile` — primary build tool. Key targets:

- `just build-release` → `cargo build --release`
- `just run` → `RUST_BACKTRACE=full cargo run --release`
- `just check` → `cargo clippy --all-features -- -W clippy::pedantic`
- `just install` → copies binary to `/usr/bin/cosmic-liquid`, desktop entry, metainfo, and SVG icon

Installed paths use RDNN `com.github.cosmix.LiquidMon`. Vendored dependency workflow available via `just build-vendored`.

## Cross-Cutting Concerns Synthesis

### The "No Real Config" Gap

The configuration infrastructure (`src/config.rs`, `src/app.rs:68-79`, `src/app.rs:187-195`) is fully wired for hot-reload, but the `Config` struct contains only a dead `demo: String` field. Two critical runtime behaviors are therefore hardcoded outside the config system:

- Device match filter: `"Hydro"` at `src/app.rs:174`
- Poll interval: `1500 ms` at `src/app.rs:180`

When these are moved to `Config`, the subscription closure in `src/app.rs:169-185` must receive them as captured values. Because `Subscription::run_with` uses a static ID string, changing the poll interval requires either restarting the subscription (new ID) or passing a channel to update the sleep duration at runtime.

### Reliability Chain from Poll to Display

The path `liquidctl subprocess → AioStatus → last_status → view` has three failure modes that compound:

1. **Subprocess hang** (`src/liquidctl.rs:109-113`): no timeout → subscription loop stalls → panel button freezes. Recovery: `tokio::time::timeout` + `kill_on_drop`.
2. **Parse failure** (`src/liquidctl.rs:133-198`): `Error::NoDevice` or `Error::Parse` → `StatusTick(Err)` → `last_error` set → panel shows `"\!"`. Old `last_status` preserved (intentional, `src/app.rs:212-215`).
3. **Startup race** (`src/app.rs:224`): `unwrap()` on `main_window_id()` panics if `TogglePopup` fires before the window is assigned.

The stale-data preservation design is intentional and correct — users see recent data with an error badge rather than a blank panel.

### i18n Infrastructure vs. Actual Usage

`src/i18n.rs` has a complete Fluent/i18n-embed pipeline: translations compiled into the binary, `DesktopLanguageRequester` in `src/main.rs:10`, and the `fl\!()` macro defined at `src/i18n.rs:44-52`. However, `src/app.rs` uses zero `fl\!()` calls — all user-visible strings are plain Rust literals. The `i18n/en/cosmic_liquid.ftl` file contains boilerplate placeholder keys (`welcome`, `example-row`, `git-description`) that are never referenced. Internationalizing the applet requires replacing every string literal in `src/app.rs:95-155` with `fl\!()` calls and adding proper message IDs to the `.ftl` file.

### libcosmic Dependency Risk

`Cargo.toml:22` pins `libcosmic` to a floating `git` HEAD. The library drives the entire framework integration: `cosmic::Application`, `cosmic::Core`, `widget::*`, `Subscription`, `Task`, the popup commands, and `cosmic_config`. Any upstream breaking change silently lands on the next `cargo fetch`. The `Cargo.lock` file provides reproducibility within a checkout, but CI and fresh installs are unguarded. The `[patch.'https://github.com/pop-os/libcosmic']` block (`Cargo.toml:30-33`) is commented out but shows the intended local-override workflow for debugging libcosmic issues.

### Module Responsibility Summary

| Module         | Responsibility                                     | External I/O                          |
| -------------- | -------------------------------------------------- | ------------------------------------- |
| `main.rs`      | Binary entry, i18n init, framework launch          | None                                  |
| `app.rs`       | AppModel, all message handling, all view rendering | libcosmic IPC, Wayland popup commands |
| `liquidctl.rs` | Subprocess invocation and JSON parsing             | `liquidctl` process via stdin/stdout  |
| `config.rs`    | Config schema declaration only                     | cosmic-config/dbus (via libcosmic)    |
| `i18n.rs`      | Translation loader, `fl\!()` macro                 | Compiled-in binary assets             |
