# Entry Points

> Key files agents should read first to understand the codebase.
> This file is append-only - agents add discoveries, never delete.

## Reading Order for New Contributors — UPDATED 2026-06-12

Read files in this order to understand the codebase end-to-end:

1. `src/main.rs` (14 lines) — Entry point. Calls `cosmic::applet::run::<AppModel>(())`. Tiny file; declares the eight module names (`app`, `config`, `devices`, `equalizer`, `liquidctl`, `sparkline`, `spinner`, `view`).
2. `src/app.rs` (~1225 lines) — Core application logic. Defines `AppModel`, the `Message` enum, and all `cosmic::Application` trait implementations including the `popup_metrics_view` orchestrator. The most important file in the codebase.
3. `src/view.rs` (~411 lines) — Stateless popup widget builders extracted from `app.rs`. Holds `ICON_*` consts, `TEMP_RANGE`/`DUTY_RANGE`, `symbolic_icon`/`symbolic_icon_sized`, `eq_canvas`, `metric_block`, `metric_value`, `fan_rows`, `spinner_glyph`, `interval_control`, `dropdown_entries`, `device_dropdown_section`, `device_dropdown_selected`, and the dropdown tests. All items are `pub(crate)`.
4. `src/liquidctl.rs` — All communication with the `liquidctl` process. Defines the public `AioStatus`, `Pump`, `Fan`, `DetectedDevice`, and `Error` types; implements `fetch_status()` and `list_devices()` async functions and the JSON parsers. All subprocess calls serialize behind a module-private `LIQUIDCTL_LOCK` mutex.
5. `src/devices.rs` (~100 lines) — AIO classification helpers. Contains the `AIO_PATTERNS` substring catalog (lowercase, narrow to families verified against the parser schema), plus pure helpers `is_aio`, `filter_aios`, and `auto_select`.
6. `src/sparkline.rs` — Iced `Canvas` widget that renders a gradient-filled sparkline from a slice of f64 samples. Used in `view()` (panel button only).
7. `src/config.rs` (~21 lines) — COSMIC config struct, `#[version = 3]`. Contains `sample_interval_ms: u64` (default 1500) and `device_match: Option<String>` (default `None` = auto-detect).

## Key Types and Their Locations — UPDATED 2026-06-12

| Type              | File               | Purpose                                                                                  |
| ----------------- | ------------------ | ---------------------------------------------------------------------------------------- |
| `AppModel`        | `src/app.rs`       | Top-level application state (14 fields incl. `anim_t`, `enumeration_retried`)            |
| `Message`         | `src/app.rs`       | All UI/async events (9 variants incl. `StatusTick{match_str,result}`, `AnimationTick`)   |
| `AioStatus`       | `src/liquidctl.rs` | Parsed snapshot from `liquidctl --json status`                                            |
| `Pump`            | `src/liquidctl.rs` | Pump speed + duty                                                                         |
| `Fan`             | `src/liquidctl.rs` | Per-fan index + speed + duty                                                              |
| `DetectedDevice`  | `src/liquidctl.rs` | Public type emitted by `list_devices` — `description`, `bus`, `address`                    |
| `Error`           | `src/liquidctl.rs` | liquidctl integration errors (six variants; `MissingField` is now a struct variant)      |
| `Config`          | `src/config.rs`    | Persisted user settings (`sample_interval_ms: u64`, `device_match: Option<String>`) — v3   |
| `Sparkline`       | `src/sparkline.rs` | Canvas widget for gradient-filled sparkline (panel button only)                          |
| `Equalizer`       | `src/equalizer.rs` | Canvas VU-meter widget for popup metric histories                                        |
| `Spinner`         | `src/spinner.rs`   | Canvas animated fan/pump glyph widget                                                    |
| `DeviceEntry`     | `src/liquidctl.rs` | Private — raw `--json status` device                                                     |
| `StatusEntry`     | `src/liquidctl.rs` | Private — raw status key/value(`serde_json::Value`)/unit                                  |
| `ListDeviceEntry` | `src/liquidctl.rs` | Private — raw `list --json` entry; tolerant string deserialization for `bus`/`address`     |

## Notable Constants and Statics — UPDATED 2026-06-12

| Identifier            | Location          | Value / Purpose                                                                  |
| --------------------- | ----------------- | -------------------------------------------------------------------------------- |
| `APP_ID`              | `src/app.rs`      | `"com.github.cosmix.LiquidMon"` — RDNN for config and desktop                   |
| `AUTOSIZE_ID`         | `src/app.rs:23`   | `LazyLock<widget::Id>` — stable ID for the autosize wrapper                     |
| `PANEL_SPARK_SAMPLES` | `src/app.rs:25`   | `60` — trailing-N window of `temp_history` fed to the panel button sparkline     |
| `HISTORY_CAP`         | `src/app.rs:26`   | `900` — maximum entries in each per-metric `VecDeque` (~15 min at 1 s polling)   |
| `MIN_INTERVAL_MS`     | `src/app.rs:27`   | `1000` — lower bound (ms) for the user-configurable sample interval              |
| `MAX_INTERVAL_MS`     | `src/app.rs:28`   | `10000` — upper bound (ms) for the user-configurable sample interval             |
| `ANIM_INTERVAL`       | `src/app.rs:33`   | `Duration::from_millis(33)` — animation tick rate and per-tick `anim_t` advance  |
| `ICON_TEMP`           | `src/view.rs:21`  | `pub(crate)` — embedded SVG bytes for temperature icon (moved from app.rs)       |
| `ICON_SNOWFLAKE`      | `src/view.rs:22`  | `pub(crate)` — embedded SVG bytes for snowflake/coolant icon                     |
| `ICON_FAN`            | `src/view.rs:24`  | `pub(crate)` — embedded SVG bytes for fan icon                                   |
| `ICON_PUMP`           | `src/view.rs:25`  | `pub(crate)` — embedded SVG bytes for pump icon                                  |
| `TEMP_RANGE`          | `src/view.rs:30`  | `pub(crate)` — `(20.0, 55.0)` °C — absolute range for coolant equalizer          |
| `DUTY_RANGE`          | `src/view.rs:31`  | `pub(crate)` — `(0.0, 100.0)` % — absolute range for pump/fan duty equalizers    |

## Critical Code Paths

### Panel button rendering

`app.rs:120-165` — `view()`. When `last_status` is present, renders a horizontal `row` containing: coolant icons (snowflake + thermometer), temperature text, sparkline canvas (36×16 px), fan icon + average fan duty%, pump icon + pump duty%. Shows `"!"` on error (no data), `"…"` while waiting.

### Popup rendering

`app.rs:170-213` — `view_window()`. Three states: (a) status available — shows device description heading, liquid temp, pump, and fan rows; (b) error only — shows error heading + message; (c) initial — shows "Waiting for first reading…".

### liquidctl polling subscription

`subscription()` builds the config-watch subscription unconditionally and appends the poll subscription only when `effective_match()` is `Some(_)`. The poll is keyed on `(u64, String) = (interval_ms, match_str)`, so changing either the user-committed interval or the selected device tears down and restarts the poll loop. Until the first `DevicesEnumerated` lands, no poll subscription exists — the panel shows `…` instead of a spurious "no AIO" error.

### liquidctl subprocess calls

`fetch_status()` sets `kill_on_drop(true)`, wraps `cmd.output()` in `tokio::time::timeout(Duration::from_secs(3))`, and returns UTF-8 stdout to `parse_status_response`. The match filter is the description string from `effective_match()`.

`list_devices()` is the parallel enumeration entry point: spawns `liquidctl list --json` with a 1 s timeout (HID enumeration only — no per-device transaction), parses into `Vec<DetectedDevice>` via `parse_devices_response`. An empty array deserializes to `Ok(vec![])`, distinct from `fetch_status`'s `Error::NoDevice`.

Both functions acquire `LIQUIDCTL_LOCK: LazyLock<tokio::sync::Mutex<()>>` at the top of their body. The lock prevents concurrent subprocess invocations from racing on the exclusive HID claim (e.g. popup-open enumerate vs. in-flight poll).

### JSON parsing

`liquidctl.rs:142-211` — `parse_status_response()`. Deserializes `Vec<DeviceEntry>`, picks the first device with a non-empty `status` array, applies bounded cast helpers (`to_u8_pct`, `to_u32`), then iterates entries matching on key strings: `"Liquid temperature"`, `"Pump speed"`, `"Pump duty"`, and `"Fan N speed"` / `"Fan N duty"` via `split_fan_key()`. Missing required fields now surface as `Error::MissingField(&'static str)` rather than `Error::NoDevice`.

### Message dispatch

`update()` handles `StatusTick(Ok)` by pushing onto three metric histories (`temp_history`, `pump_duty_history`, `fan_avg_duty_history`) capped at `HISTORY_CAP` via `push_capped`, replacing `last_status`, and clearing `last_error`. The fan-average duty push is skipped when no fans are reported (pushing 0.0 would corrupt the auto-scaled y-axis). `StatusTick(Err)` sets `last_error` but intentionally preserves stale `last_status` for display alongside the error. `SampleIntervalDragged` stages `pending_interval_secs` without touching config. `SampleIntervalReleased` calls `commit_pending_interval` which clamps, persists, and clears the staged value.

`DevicesEnumerated(Ok(devs))` snapshots `effective_match` before and after replacing `detected_devices`; if the effective device changed, it calls `reset_device_state()` which clears all three histories plus `last_status`/`last_error`. If no AIO is detected, sets `last_error` to a guidance string. `DevicesEnumerated(Err(msg))` writes `last_error` and clears `device_scan_in_flight` without touching status state. `DeviceSelected(choice)` short-circuits when `config.device_match == choice`; otherwise mirrors the same prev/new effective-match diff and persists via `config.write_entry(handle)` — semantic no-ops (e.g. explicit pick of the auto device) do not reset history.

The `TogglePopup` open branch now batches `get_popup` with a fresh `list_devices` Task (gated on `!device_scan_in_flight`) so opening the popup re-enumerates hot-plugged devices.

### Popup toggle

`app.rs:270-293` — `TogglePopup` arm. Guards with `let Some(parent) = self.core.main_window_id() else { ... }` to avoid panicking if the window is not yet assigned. Creates a new `Id::unique()`, calls `get_popup()` with size limits (300–372 px wide, 200–1080 px tall), or calls `destroy_popup()` if already open.

## Where to Add New Features

| Feature                   | File to edit                  | Notes                                                                   |
| ------------------------- | ----------------------------- | ----------------------------------------------------------------------- |
| New config option         | `src/config.rs`               | Add field, increment `#[version = N]`, update hand-implemented `Default`            |
| New status metric         | `src/liquidctl.rs`            | Add match arm to the key loop in `parse_status_response`; update `AioStatus`        |
| New popup metric section  | `src/app.rs:popup_metrics_view` | Add `metric_section(...)` call between heading and slider                        |
| Panel button elements     | `src/app.rs:view`             | Modify the `row![]` in the `Some(status)` arm                                       |
| New async background task | `src/app.rs:subscription`     | Append to the `subs` Vec; key on whatever data should restart the stream            |
| New AIO family pattern    | `src/devices.rs:AIO_PATTERNS` | Add a lowercase substring; pair with parser-schema verification                    |

## Test Coverage — UPDATED 2026-06-12

Tests live in `#[cfg(test)] mod tests` blocks at the bottom of each relevant file. **109 unit tests total.** Run with `cargo test`.

Per-module counts (verified): `app`=40, `liquidctl`=30, `view`=11, `equalizer`=10, `sparkline`=9, `devices`=6, `spinner`=3.

### `src/liquidctl.rs` — parser tests (30)

Fixture and error-path coverage including:

- `parses_h150i_pro_xt_fixture` — full parse of a real H150i Pro XT JSON snapshot
- `empty_array_yields_no_device`, `all_devices_empty_status_yields_no_device` — `Error::NoDevice` paths
- `device_missing_liquid_temp_yields_missing_field` etc. — required-field absence yields `Error::MissingField{field,device}`
- `fan_with_only_speed_is_dropped`, `fan_with_only_duty_is_dropped` — fans missing one of speed/duty are filtered
- `fan_index_zero_is_ignored`, `fans_emerge_sorted_by_index` — index policy
- `out_of_range_pump_duty_is_clamped`, `negative_values_clamp_to_zero` — cast-helper bounds
- `first_device_with_status_is_selected`, `unknown_keys_are_silently_ignored`, `malformed_json_yields_parse_error`
- `split_fan_key_*`, `display_*`, `error_source_chains_*` — helper and Display tests

### `src/app.rs` — model tests (40)

Helper and `update()` state-transition tests. Constructs model via `AppModel::default()`:

- `fan_duty_avg_*` — `fan_duty_avg` rounds to nearest (not truncating)
- `fan_speed_avg_*` — `fan_speed_avg` helper
- `status_tick_ok_*`, `status_tick_err_*`, `temp_history_caps_*`, `status_tick_with_no_fans_*`
- `sample_interval_*` — drag/release/clamp/noop cases
- `popup_closed_*`, `update_config_*`

### `src/view.rs` — dropdown tests (11)

Tests for `dropdown_entries`, `device_dropdown_items`, `device_dropdown_selected`:

- `dropdown_items_includes_auto_first`, `dropdown_items_appends_disconnected_synthetic_when_saved_missing`, `dropdown_items_omits_synthetic_when_saved_is_connected`, `dropdown_items_omits_auto_picked_device_from_explicit_list`, `dropdown_items_lists_non_auto_aios_explicitly`
- `dropdown_omits_disconnected_for_connected_non_aio_saved` — gated on full device list
- `dropdown_commits_real_value_not_display_label`
- `dropdown_selected_*` — selected-index resolution cases

### `src/equalizer.rs` — canvas tests (10), `src/sparkline.rs` — canvas tests (9), `src/devices.rs` — classification tests (6), `src/spinner.rs` — animation tests (3)

Not covered: `view`/`view_window` rendering, `subscription`, the `TogglePopup` arm (touches `core.main_window_id()`), and `fetch_status`'s subprocess invocation.

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

## CI/CD Entry Points — UPDATED 2026-06-12

The workflow files are present on disk at `.github/workflows/ci.yml` and `.github/workflows/release.yml`. The earlier entry claiming they were "only in git history" was stale.

New contributors should understand:

- `just ci-local` mirrors the CI `check` job exactly (fmt + clippy `-D warnings` + test + release build)
- `just check` is the PEDANTIC local lint (adds `-W clippy::pedantic`); it is advisory only — NOT the CI gate
- CI runs on push to `main` and all PRs — has a separate `audit` job for `cargo audit`
- Releases are tag-driven: push `v*` tag → `.deb` + tarball + SHA256SUMS → GitHub release; release.yml verifies Cargo.toml version matches tag
- Use `just tag <version>` to bump version (semver guard), commit, and tag in one step

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
