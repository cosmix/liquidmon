# Entry Points

> Key files agents should read first to understand the codebase.
> This file is append-only - agents add discoveries, never delete.

## Reading Order for New Contributors

Read files in this order to understand the codebase end-to-end:

1. `src/main.rs` (10 lines) — Entry point. Calls `cosmic::applet::run::<AppModel>(())`. Tiny file; establishes the four module names.
2. `src/app.rs` (307 lines) — Core application logic. Defines `AppModel`, the `Message` enum, and all `cosmic::Application` trait implementations. The most important file in the codebase.
3. `src/liquidctl.rs` (294 lines) — All communication with the `liquidctl` process. Defines the public `AioStatus`, `Pump`, `Fan`, and `Error` types; implements `fetch_status()` async function and the JSON parser. Contains the unit test suite.
4. `src/sparkline.rs` (72 lines) — Iced `Canvas` widget that renders a temperature sparkline from a slice of f64 samples. Used at `src/app.rs:130`.
5. `src/config.rs` (7 lines) — COSMIC config struct. Currently empty (`pub struct Config {}`); the place to add persistent user preferences.

## Key Types and Their Locations

| Type          | File               | Lines  | Purpose                                        |
| ------------- | ------------------ | ------ | ---------------------------------------------- |
| `AppModel`    | `src/app.rs`       | 44-58  | Top-level application state                    |
| `Message`     | `src/app.rs`       | 61-67  | All UI/async events                            |
| `AioStatus`   | `src/liquidctl.rs` | 12-18  | Parsed snapshot from liquidctl                 |
| `Pump`        | `src/liquidctl.rs` | 20-24  | Pump speed + duty                              |
| `Fan`         | `src/liquidctl.rs` | 26-31  | Per-fan speed + duty                           |
| `Error`       | `src/liquidctl.rs` | 33-44  | liquidctl integration errors (six variants)    |
| `Config`      | `src/config.rs`    | 5-7    | Persisted user settings (empty struct)         |
| `Sparkline`   | `src/sparkline.rs` | 1-72   | Canvas widget for panel temperature sparkline  |
| `DeviceEntry` | `src/liquidctl.rs` | 95-103 | Raw JSON device from liquidctl                 |
| `StatusEntry` | `src/liquidctl.rs` | 105-111| Raw JSON status key/value/unit                 |

## Notable Constants and Statics

| Identifier      | Location         | Value / Purpose                                               |
| --------------- | ---------------- | ------------------------------------------------------------- |
| `APP_ID`        | `src/app.rs:81`  | `"com.github.cosmix.LiquidMon"` — RDNN for config and desktop |
| `AUTOSIZE_ID`   | `src/app.rs:19`  | `LazyLock<widget::Id>` — stable ID for the autosize wrapper   |
| `MAX_SAMPLES`   | `src/app.rs:22`  | `60` — sparkline history depth (~90 s at 1500 ms/sample)      |
| `ICON_TEMP`     | `src/app.rs:23`  | Embedded SVG bytes for temperature icon                       |
| `ICON_SNOWFLAKE`| `src/app.rs:24`  | Embedded SVG bytes for snowflake/coolant icon                 |
| `ICON_FAN`      | `src/app.rs:25`  | Embedded SVG bytes for fan icon                               |
| `ICON_PUMP`     | `src/app.rs:26`  | Embedded SVG bytes for pump icon                              |

## Critical Code Paths

### Panel button rendering

`app.rs:120-165` — `view()`. When `last_status` is present, renders a horizontal `row` containing: coolant icons (snowflake + thermometer), temperature text, sparkline canvas (36×16 px), fan icon + average fan duty%, pump icon + pump duty%. Shows `"!"` on error (no data), `"…"` while waiting.

### Popup rendering

`app.rs:170-213` — `view_window()`. Three states: (a) status available — shows device description heading, liquid temp, pump, and fan rows; (b) error only — shows error heading + message; (c) initial — shows "Waiting for first reading…".

### liquidctl polling subscription

`app.rs:221-246` — `subscription()`. Runs a background async stream (`Subscription::run_with`) that calls `fetch_status("Hydro")` in an infinite loop with 1500 ms sleep. Also subscribes to config changes via `core.watch_config`.

### liquidctl subprocess call

`liquidctl.rs:115-138` — `fetch_status()`. Sets `kill_on_drop(true)`, wraps `cmd.output()` in `tokio::time::timeout(Duration::from_secs(3))`, checks exit code, returns UTF-8 stdout to the parser. Match filter is `"Hydro"` (hardcoded at `app.rs:229`).

### JSON parsing

`liquidctl.rs:142-211` — `parse_status_response()`. Deserializes `Vec<DeviceEntry>`, picks the first device with a non-empty `status` array, applies bounded cast helpers (`to_u8_pct`, `to_u32`), then iterates entries matching on key strings: `"Liquid temperature"`, `"Pump speed"`, `"Pump duty"`, and `"Fan N speed"` / `"Fan N duty"` via `split_fan_key()`. Missing required fields now surface as `Error::MissingField(&'static str)` rather than `Error::NoDevice`.

### Message dispatch

`app.rs:253-302` — `update()`. Handles `StatusTick(Ok)` by pushing temp onto `temp_history` (capped at `MAX_SAMPLES`), replacing `last_status`, and clearing `last_error`. `StatusTick(Err)` sets `last_error` but intentionally preserves stale `last_status` for display alongside the error (`app.rs:268`).

### Popup toggle

`app.rs:270-293` — `TogglePopup` arm. Guards with `let Some(parent) = self.core.main_window_id() else { ... }` to avoid panicking if the window is not yet assigned. Creates a new `Id::unique()`, calls `get_popup()` with size limits (300–372 px wide, 200–1080 px tall), or calls `destroy_popup()` if already open.

## Where to Add New Features

| Feature                   | File to edit                  | Notes                                                    |
| ------------------------- | ----------------------------- | -------------------------------------------------------- |
| New config option         | `src/config.rs`               | Add field, increment `VERSION`                           |
| New status metric         | `src/liquidctl.rs:166-183`    | Add match arm to the key loop; update `AioStatus` struct |
| New popup row             | `src/app.rs:175-193`          | Add `column.add(...)` in the `Some(status)` branch       |
| Panel button elements     | `src/app.rs:120-165`          | Modify the `row![]` in the `Some(status)` arm            |
| New async background task | `src/app.rs:221-246`          | Add to `Subscription::batch` vector                      |

## Test Coverage

Tests live in two `#[cfg(test)] mod tests` blocks: one at the bottom of `src/liquidctl.rs` and one at the bottom of `src/app.rs`. 30 unit tests total. Run with `cargo test`.

### `src/liquidctl.rs` — parser tests (20)

Fixture and error-path coverage:

- `parses_h150i_pro_xt_fixture` — full parse of a real H150i Pro XT JSON snapshot; verifies temperature, pump, and all three fans
- `empty_array_yields_no_device`, `all_devices_empty_status_yields_no_device` — `Error::NoDevice` paths
- `device_missing_liquid_temp_yields_missing_field`, `device_missing_pump_speed_yields_missing_field`, `device_missing_pump_duty_yields_missing_field` — required-field absence yields `Error::MissingField(<name>)`
- `fan_with_only_speed_is_dropped`, `fan_with_only_duty_is_dropped` — fans missing one of speed/duty are filtered out (filter at `liquidctl.rs:190-200`)
- `fan_index_zero_is_ignored` — `Fan 0` keys silently dropped per `split_fan_key`
- `fans_emerge_sorted_by_index` — shuffled-input fans come out ordered (BTreeMap + explicit sort)
- `out_of_range_pump_duty_is_clamped`, `negative_values_clamp_to_zero` — `to_u8_pct`/`to_u32` bounds
- `first_device_with_status_is_selected` — multi-device selection picks first non-empty `status`
- `unknown_keys_are_silently_ignored` — extraneous keys (e.g. `Firmware version`) don't break parsing
- `malformed_json_yields_parse_error` — invalid JSON → `Error::Parse`
- `split_fan_key_extracts_index_and_suffix`, `split_fan_key_rejects_zero_and_malformed` — direct unit tests of the private helper
- `display_includes_field_name_for_missing_field`, `display_for_no_device_and_timeout` — `Display` impl smoke tests
- `error_source_chains_for_inner_io_and_parse` — `std::error::Error::source()` chains for `Spawn`/`Parse`, returns `None` for `NoDevice`/`MissingField`/`Timeout`

### `src/app.rs` — model tests (10)

Helper and `update()` state-transition tests. The test module imports `cosmic::Application as _` to bring the trait method `update` into scope, and constructs `AppModel` via `AppModel::default()` (which works because `cosmic::Core: Default` and all other fields derive `Default`):

- `fan_duty_avg_is_none_for_empty`, `fan_duty_avg_computes_integer_mean`, `fan_duty_avg_truncates_toward_zero`, `fan_duty_avg_at_max` — pure helper at `app.rs:34-40`
- `status_tick_ok_appends_temp_and_clears_error` — `StatusTick(Ok)` pushes to `temp_history`, sets `last_status`, clears `last_error`
- `status_tick_err_preserves_stale_status` — `StatusTick(Err)` sets `last_error` but does NOT clear `last_status` (the deliberate stale-data design at `app.rs:274`)
- `temp_history_caps_at_max_samples` — pushing `MAX_SAMPLES + 10` samples leaves exactly `MAX_SAMPLES = 60`, with the oldest dropped from the front
- `popup_closed_with_matching_id_clears_popup`, `popup_closed_with_non_matching_id_is_noop` — `PopupClosed(Id)` only clears when the id matches
- `update_config_replaces_config` — `UpdateConfig(Config)` arm runs without disturbing other state

Not covered: `view`/`view_window` rendering, `subscription`, the `TogglePopup` arm (touches `core.main_window_id()`), and `fetch_status`'s subprocess invocation (only the pure `parse_status_response` is exercised).

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

- `src/app.rs:81` (APP_ID constant)
- `justfile:2` (appid variable, drives all install paths)
- `resources/app.desktop:5` (Icon), `resources/app.desktop:1` (desktop file name)
- `resources/app.metainfo.xml:3` (component id)
- COSMIC config storage path (managed by libcosmic/cosmic-settings-daemon)

Changing the app ID requires updating all four locations plus reinstalling.

## CI/CD Entry Points

The workflow files exist only in git history (committed in `6f9b43b`, not currently on-disk). To review them:

```text
git show 6f9b43b:.github/workflows/ci.yml
git show 6f9b43b:.github/workflows/release.yml
```

New contributors should understand:

- CI runs on push to `main` and all PRs — fmt, clippy (pedantic, warnings-as-errors), test, release build
- Releases are tag-driven: push `v*` tag → `.deb` + tarball + SHA256SUMS → GitHub release
- Use `just tag <version>` to bump version, commit, and tag in one step

## Resources Directory — Complete File List

```text
resources/
├── app.desktop          # XDG desktop entry (installed to share/applications/)
├── app.metainfo.xml     # AppStream metadata (installed to share/appdata/)
├── icon.svg             # Main app icon (installed to hicolor/scalable/apps/)
└── icons/
    ├── fan-symbolic.svg          # Fan speed symbolic icon
    ├── pump-symbolic.svg         # Pump duty symbolic icon
    ├── snowflake-symbolic.svg    # Cooling indicator symbolic icon
    └── temperature-symbolic.svg  # Temperature symbolic icon
```

The four symbolic icons in `resources/icons/` are the COSMIC-style inline icons embedded in the applet's panel button and popup widget. They follow the freedesktop symbolic icon naming convention (suffix `-symbolic`).

The `resources/icon.svg` (app icon) is embedded via the `appid` variable in justfile: installed as `com.github.cosmix.LiquidMon.svg`.
