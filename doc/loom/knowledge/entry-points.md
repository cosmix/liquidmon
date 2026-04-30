# Entry Points

> Key files agents should read first to understand the codebase.
> This file is append-only - agents add discoveries, never delete.

(Add entry points as you discover them)

## Reading Order for New Contributors

Read files in this order to understand the codebase end-to-end:

1. `src/main.rs` (17 lines) — Entry point. Initializes i18n, then calls `cosmic::applet::run::<AppModel>(())`. Tiny file; establishes the four module names.
2. `src/app.rs` (251 lines) — Core application logic. Defines `AppModel`, the `Message` enum, and all `cosmic::Application` trait implementations. The most important file in the codebase.
3. `src/liquidctl.rs` (267 lines) — All communication with the `liquidctl` process. Defines the public `AioStatus`, `Pump`, `Fan`, and `Error` types; implements `fetch_status()` async function and the JSON parser. Contains the unit test suite.
4. `src/config.rs` (9 lines) — COSMIC config struct. Currently a stub with a single `demo: String` field; the place to add persistent user preferences.
5. `src/i18n.rs` (53 lines) — i18n scaffolding. Embeds locale files, sets up the Fluent loader, and defines the `fl\!()` macro. Can be ignored until localization work begins.

## Key Types and Their Locations

| Type          | File               | Lines  | Purpose                        |
| ------------- | ------------------ | ------ | ------------------------------ |
| `AppModel`    | `src/app.rs`       | 16-27  | Top-level application state    |
| `Message`     | `src/app.rs`       | 31-36  | All UI/async events            |
| `AioStatus`   | `src/liquidctl.rs` | 12-17  | Parsed snapshot from liquidctl |
| `Pump`        | `src/liquidctl.rs` | 19-22  | Pump speed + duty              |
| `Fan`         | `src/liquidctl.rs` | 24-29  | Per-fan speed + duty           |
| `Error`       | `src/liquidctl.rs` | 32-41  | liquidctl integration errors   |
| `Config`      | `src/config.rs`    | 6-9    | Persisted user settings (stub) |
| `DeviceEntry` | `src/liquidctl.rs` | 89-96  | Raw JSON device from liquidctl |
| `StatusEntry` | `src/liquidctl.rs` | 98-104 | Raw JSON status key/value/unit |

## Critical Code Paths

### Panel button rendering

`app.rs:95-110` — `view()`. Shows `"{temp:.1}°"` when data is available, `"\!"` on error, `"…"` while waiting.

### Popup rendering

`app.rs:115-158` — `view_window()`. Three states: (a) status available — shows device description, liquid temp, pump, and fan rows; (b) error only — shows error heading + message; (c) initial — shows "Waiting for first reading…".

### liquidctl polling subscription

`app.rs:166-197` — `subscription()`. Runs a background async stream (`Subscription::run_with`) that calls `fetch_status("Hydro")` in an infinite loop with 1500 ms sleep. Also subscribes to config changes via `core.watch_config`.

### liquidctl subprocess call

`liquidctl.rs:108-129` — `fetch_status()`. Spawns `liquidctl --match <filter> --json status`, checks exit code, returns UTF-8 stdout to the parser. Match filter is `"Hydro"` (hardcoded at `app.rs:174`).

### JSON parsing

`liquidctl.rs:133-199` — `parse_status_response()`. Deserializes `Vec<DeviceEntry>`, picks the first device with a non-empty `status` array, then iterates entries matching on key strings: `"Liquid temperature"`, `"Pump speed"`, `"Pump duty"`, and `"Fan N speed"` / `"Fan N duty"` via `split_fan_key()`.

### Message dispatch

`app.rs:204-245` — `update()`. Handles `StatusTick(Ok)` by replacing `last_status`; `StatusTick(Err)` sets `last_error` but intentionally preserves stale `last_status` for display alongside the error (`app.rs:214-216`).

### Popup toggle

`app.rs:217-236` — `TogglePopup` arm. Creates a new `Id::unique()`, calls `get_popup()` with size limits (300-372 px wide, 200-1080 px tall), or calls `destroy_popup()` if already open.

## Where to Add New Features

| Feature                   | File to edit                | Notes                                                    |
| ------------------------- | --------------------------- | -------------------------------------------------------- |
| New config option         | `src/config.rs`             | Add field, increment `VERSION`                           |
| New status metric         | `src/liquidctl.rs:148-171`  | Add match arm to the key loop; update `AioStatus` struct |
| New popup row             | `src/app.rs:119-141`        | Add `column.add(...)` in the `Some(status)` branch       |
| Panel button text         | `src/app.rs:96-103`         | Change the label format string                           |
| Localized strings         | `i18n/en/cosmic_liquid.ftl` | Add key, use `fl\!("key")` in app.rs                     |
| New async background task | `src/app.rs:166-197`        | Add to `Subscription::batch` vector                      |

## Test Coverage

All tests are in `src/liquidctl.rs:213-267`. Three unit tests:

- `parses_h150i_pro_xt_fixture` — full parse of a real device JSON snapshot for the Corsair Hydro H150i Pro XT; verifies temperature, pump, and all three fans
- `empty_array_yields_no_device` — empty JSON array → `Error::NoDevice`
- `all_devices_empty_status_yields_no_device` — multiple devices all with empty status arrays → `Error::NoDevice`

Run with: `cargo test`

## Build and Development Workflow

```text
# First-time setup
sudo ./scripts/install-liquidctl-udev.sh   # install HID udev rules
pip install liquidctl                       # or system package

# Development
just run              # cargo run --release with RUST_BACKTRACE=full
just check            # clippy --all-features --pedantic
cargo test            # unit tests (no device required)

# Install to /usr (requires sudo or prefix override)
just build-release
sudo just install

# Install to custom prefix (e.g., ~/.local)
just install rootdir=$HOME/.local
```

## App ID and RDNN

`com.github.cosmix.LiquidMon` — appears in:

- `src/app.rs:50` (APP_ID constant)
- `justfile:2` (appid variable, drives all install paths)
- `resources/app.desktop:5` (Icon), `resources/app.desktop:1` (desktop file name)
- `resources/app.metainfo.xml:3` (component id)
- COSMIC config storage path (managed by libcosmic/cosmic-settings-daemon)

Changing the app ID requires updating all four locations plus reinstalling.
