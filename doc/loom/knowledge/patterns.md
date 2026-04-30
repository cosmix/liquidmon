# Architectural Patterns

> Discovered patterns in the codebase that help agents understand how things work.
> This file is append-only - agents add discoveries, never delete.

## Error Handling Patterns

### Domain error enum with Display and std::error::Error (liquidctl.rs:33-92)

`liquidctl.rs` defines a custom `Error` enum with six variants covering all failure modes of the liquidctl CLI integration:

```text
Error::Spawn(io::Error)               — process could not be spawned
Error::NonZeroExit { status, stderr } — process ran but failed
Error::Parse(serde_json::Error)       — stdout was valid UTF-8 but bad JSON
Error::NoDevice                       — JSON parsed but no device with non-empty status
Error::MissingField(&'static str)     — device found but required field absent (e.g. "liquid temperature")
Error::Timeout                        — subprocess did not complete within the 3 s deadline
```

`Display` is hand-implemented with match arms that produce human-readable messages (liquidctl.rs:46-70). `std::error::Error::source()` is implemented to chain underlying `io::Error` or `serde_json::Error` causes (liquidctl.rs:72-80); the `_ => None` arm covers `NoDevice`, `MissingField`, and `Timeout` since none carry an inner source. `From<io::Error>` and `From<serde_json::Error>` allow `?` propagation inside `fetch_status` (liquidctl.rs:82-92).

### MissingField carries a &'static str field name (liquidctl.rs:42)

`Error::MissingField(&'static str)` carries the name of the missing field as a string literal (e.g. `"liquid temperature"`, `"pump speed"`, `"pump duty"`). This distinguishes a device that was found but had incomplete data from one that was never found (`Error::NoDevice`). The static string allows the variant to be `Debug`-printed and matched in tests without allocation.

### Bounded cast helpers for f64 → integer conversion (liquidctl.rs:150-152)

Before casting sensor values, two inline closures enforce domain limits:

```rust
let to_u8_pct = |v: f64| v.clamp(0.0, 100.0) as u8;
let to_u32    = |v: f64| v.clamp(0.0, u32::MAX as f64) as u32;
```

These are applied at every cast site (pump speed, pump duty, fan speed, fan duty). This prevents silent wrap-around from out-of-range liquidctl values.

### Bounded subprocess timeout with kill-on-drop (liquidctl.rs:116-122)

`fetch_status` guards against subprocess hangs with two complementary mechanisms:

```rust
cmd.kill_on_drop(true);
let output = tokio::time::timeout(Duration::from_secs(3), cmd.output())
    .await
    .map_err(|_| Error::Timeout)?
    .map_err(Error::Spawn)?;
```

`kill_on_drop(true)` ensures that if the `Command` handle is dropped (e.g. because the `timeout` future is cancelled), the child process is sent `SIGKILL` rather than becoming an orphan. The `timeout` wraps the entire `.output()` future, so the 3 s deadline covers both process startup and I/O completion. If the deadline elapses, `Error::Timeout` is returned and the child is killed. This combo prevents the subscription loop from stalling indefinitely when the USB device hangs.

### ? propagation with map_err for type conversion (liquidctl.rs:119-136)

`fetch_status` uses `.map_err(|_| Error::Timeout)?` for the timeout layer, `.map_err(Error::Spawn)?` for the spawn/IO layer, and an explicit closure to wrap a UTF-8 decode error as `Error::Spawn(io::Error::new(...))` (liquidctl.rs:131-136). The serde_json parse step uses plain `?` via the `From` impl.

### Lossy error conversion at the app boundary (app.rs:62, 231)

At the Elm message boundary, `Result<AioStatus, Error>` is converted to `Result<AioStatus, String>` via `.map_err(|e| format!("{e}"))` before being wrapped in `Message::StatusTick` (app.rs:231). This erases error structure but keeps a human-readable description for display.

### Stale-data preservation on error (app.rs:266-269)

In `update`, the `StatusTick(Err(msg))` arm sets `last_error` but intentionally does NOT clear `last_status` — an explicit design decision documented with an in-code comment. This means the popup continues showing the last good reading alongside the error message.

### Config load with error degradation (app.rs:99-104)

Config loading uses a nested `map`/`match` chain: `cosmic_config::Config::new(...)` maps over a successful context, tries `Config::get_entry`, and on `Err((_errors, config))` discards the errors and falls back to the partial config. Then `.unwrap_or_default()` handles the case where no config context exists at all.

---

## State Management Patterns

### Single flat AppModel struct (app.rs:44-58)

All application state lives in one struct with six fields:

- `core: cosmic::Core` — runtime-managed, accessed via required trait methods
- `popup: Option<Id>` — popup window identity, `None` when closed
- `config: Config` — persisted config reloaded via subscription
- `last_status: Option<AioStatus>` — last successful poll result
- `last_error: Option<String>` — last error string, coexists with `last_status`
- `temp_history: VecDeque<f64>` — liquid temp samples for sparkline, capped at `MAX_SAMPLES = 60`

`#[derive(Default)]` is present so `init` can use struct update syntax (`..Default::default()`).

### Explicit Option states for status/error (app.rs:171-212)

`view_window` uses a three-way match on `(&self.last_status, &self.last_error)`:

- `(Some(status), maybe_err)` — display data, optionally annotate with error caption
- `(None, Some(err))` — full error state, no data yet
- `(None, None)` — initial loading state, show placeholder text

This pattern encodes "stale + error" distinctly from "no data + error".

### Toggle idiom for popup state (app.rs:270-293)

`Message::TogglePopup` uses `self.popup.take()` to atomically remove the current id — if `Some`, destroy the popup; if `None`, create a new one with `Id::unique()` and store it. A `let Some(parent) = self.core.main_window_id() else { ... }` guard prevents panicking if the compositor has not yet assigned a window.

---

## Async Patterns

### Subscription-based background polling (app.rs:221-246)

The subscription uses `Subscription::batch` containing two subscriptions:

1. **liquidctl poll** (`app.rs:224-240`): `Subscription::run_with("liquidctl-sub", ...)` wraps `cosmic::iced::stream::channel(4, ...)` — a channel with buffer size 4. The async closure runs an infinite loop: call `fetch_status`, send result, sleep 1500 ms. If the send fails (channel closed), it breaks and then awaits `futures_util::future::pending()` to keep the future alive.

2. **Config watch** (`app.rs:241-244`): `self.core().watch_config::<Config>(APP_ID).map(...)` maps config update events to `Message::UpdateConfig`.

### tokio::process::Command for subprocess (liquidctl.rs:116-122)

`tokio::process::Command::new("liquidctl")` is used with `.kill_on_drop(true)` and `.output().await` wrapped in a 3 s `tokio::time::timeout`. This captures both stdout and stderr without streaming. The async function is called from within the iced stream channel closure using `await`.

### Channel and SinkExt for message production (app.rs:227, 232-233)

The stream closure receives `mpsc::Sender<Message>` and calls `channel.send(...).await` from `futures_util::SinkExt`. The `.is_err()` check on send is the only way to detect that the applet is shutting down.

---

## UI Construction Patterns

### Panel button view as a row of icons and text (app.rs:120-165)

`view()` produces a `widget::button::custom(content)` wrapped in `autosize::autosize(button, AUTOSIZE_ID.clone())`. When status is available, `content` is a horizontal `row![]` with:

- A coolant glyph sub-row (snowflake + thermometer SVG icons)
- Temperature text via `self.core.applet.text(temp_text)`
- A `Canvas::new(Sparkline::new(...))` at 36×16 px
- Fan SVG icon + average fan duty% text
- Pump SVG icon + pump duty% text

Icons are loaded from embedded SVG bytes via `symbolic_icon()` helper (app.rs:28-32). `AUTOSIZE_ID` is a `LazyLock<widget::Id>` stable across renders.

### Popup content via list_column builder (app.rs:171-212)

`view_window` builds popup content using `widget::list_column()` with chained `.add(...)` calls. The column is built imperatively:

- Always adds a heading row from `status.description`
- Then fixed rows for liquid temp and pump
- Then a dynamic loop over `status.fans` adding one row per fan
- Then an optional error caption if `maybe_err.is_some()`

The column is converted to an `Element` via `.into()` at each match arm exit. The popup is wrapped with `self.core.applet.popup_container(content).into()`.

### Text hierarchy helpers (app.rs:175-193)

Three text widget helpers are used for typographic hierarchy:

- `widget::text::heading(...)` — device description
- `widget::text::body(...)` — sensor readings
- `widget::text::caption(...)` — error annotation

---

## Message/Command (Elm-like) Patterns

### Message enum with four variants (app.rs:61-67)

```text
Message::TogglePopup           — UI interaction (no data)
Message::PopupClosed(Id)       — window manager event
Message::UpdateConfig(Config)  — config subscription event
Message::StatusTick(Result<AioStatus, String>) — poll result
```

All variants derive `Debug` and `Clone`. The `Result` inside `StatusTick` is already stringified.

### update returns Task::none() except for popup toggle (app.rs:253-302)

All message arms except `TogglePopup` end with `Task::none()`. Only `TogglePopup` returns an actual task (`destroy_popup` or `get_popup`), using an early `return` to short-circuit the trailing `Task::none()`.

### init returns Task::none() (app.rs:108)

`init` does no async work; the subscription drives all background activity.

---

## Parsing Patterns (liquidctl.rs)

### Two-phase JSON parsing (liquidctl.rs:142-211)

Phase 1: deserialize to `Vec<DeviceEntry>` (private structs). Phase 2: iterate `device.status` and extract fields by `entry.key` string matching into local `Option` accumulators. Unknown keys are silently ignored (liquidctl.rs:181). Fan entries are accumulated into a `BTreeMap<u8, (Option<u32>, Option<u8>)>` keyed by fan index for natural ordering.

### Private deserialization structs separate from public API types (liquidctl.rs:95-111)

`DeviceEntry` and `StatusEntry` are private structs used only for deserialization. Public types `AioStatus`, `Pump`, `Fan` are the parsed, typed representation. Fields unused in parsing are annotated `#[allow(dead_code)]`.

### BTreeMap for ordered fan accumulation (liquidctl.rs:158-200)

`BTreeMap` is chosen over `HashMap` so fans are naturally ordered by index during `.into_iter()`, avoiding an explicit sort (though an explicit `.sort_by_key` is still called as belt-and-suspenders at liquidctl.rs:201).

## Sparkline Canvas Widget (src/sparkline.rs)

`Sparkline` is a struct holding `samples: Vec<f64>` constructed from any `IntoIterator<Item = f64>` (sparkline.rs:9-18). It implements `canvas::Program<Message, Theme>` with `type State = ()` — no mutable canvas state (sparkline.rs:21-22).

The `draw` method (sparkline.rs:24-71):

- Returns early with an empty frame if fewer than 2 samples are available (sparkline.rs:34-36).
- Uses a **fixed y-axis domain** of 10–40 °C (const Y_MIN/Y_MAX), intentionally not auto-scaled to the sample range. Values outside this band visually pin at the top or bottom edge — a deliberate design choice documented with a comment (sparkline.rs:38-42).
- Computes a 1 px `pad` on each edge; `usable_w` and `usable_h` clamp to at least 1.0 via `.max(1.0)` (sparkline.rs:46-48).
- Maps sample index `i` to x: `pad + (i / (n-1)) * usable_w`; maps sample value to y: `pad + (1 - norm) * usable_h`, where `norm = (sample - Y_MIN) / range` and y is inverted so higher values render higher in the frame (sparkline.rs:51-61).
- Builds a `Path` via `Path::new(|p| { ... })` using `p.move_to` for the first point and `p.line_to` for subsequent points (sparkline.rs:50-62).
- Strokes the path with `Color::from_rgba8(180, 200, 230, 0.85)` (light blue, slightly transparent) at width 1.5 (sparkline.rs:64-68).
- Returns `vec\![frame.into_geometry()]` — single geometry element per draw call.

The widget is embedded in `view()` as `Canvas::new(Sparkline::new(...))` with explicit `.width(Length::Fixed(36.0)).height(Length::Fixed(16.0))` (app.rs:130-132).

## LazyLock for Stable Widget IDs (src/app.rs:19-20)

```rust
static AUTOSIZE_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("liquidmon-applet"));
```

`widget::Id` is not `const`-constructible; a `static LazyLock` initialises it once on first access. The same `AUTOSIZE_ID.clone()` is passed to `autosize::autosize(button, AUTOSIZE_ID.clone())` (app.rs:164) every render cycle, giving the autosize wrapper a stable identity across redraws. Using `LazyLock` (from `std::sync`) is preferred over `once_cell::sync::Lazy` — no third-party dependency needed.

## include_bytes\! for Embedded SVG Icons (src/app.rs:23-26)

```rust
const ICON_TEMP: &[u8]      = include_bytes\!("../resources/icons/temperature-symbolic.svg");
const ICON_SNOWFLAKE: &[u8] = include_bytes\!("../resources/icons/snowflake-symbolic.svg");
const ICON_FAN: &[u8]       = include_bytes\!("../resources/icons/fan-symbolic.svg");
const ICON_PUMP: &[u8]      = include_bytes\!("../resources/icons/pump-symbolic.svg");
```

SVG icon files are embedded at compile time as `&'static [u8]` constants via `include_bytes\!`. This avoids runtime filesystem access and ensures icons are always available. The path is relative to `src/app.rs`, so the macro resolves to `resources/icons/` at the crate root.

## symbolic_icon() Helper (src/app.rs:28-32)

```rust
fn symbolic_icon(bytes: &'static [u8]) -> widget::icon::Icon {
    let mut handle = widget::icon::from_svg_bytes(bytes);
    handle.symbolic = true;
    widget::icon::icon(handle).size(14)
}
```

Takes a `&'static [u8]` (an embedded SVG), creates a mutable icon handle via `widget::icon::from_svg_bytes`, sets `handle.symbolic = true` so the icon is recoloured to match the panel text colour (COSMIC symbolic icon convention), then wraps it in a `widget::icon::icon(handle).size(14)` widget. All four icons (temperature, snowflake, fan, pump) are rendered this way.

## fan_duty_avg() Computation (src/app.rs:34-40)

```rust
fn fan_duty_avg(fans: &[crate::liquidctl::Fan]) -> Option<u8> {
    if fans.is_empty() {
        return None;
    }
    let sum: u32 = fans.iter().map(|f| u32::from(f.duty_pct)).sum();
    Some((sum / fans.len() as u32) as u8)
}
```

Returns `None` if the fan slice is empty (no fans reported). Otherwise widens `duty_pct: u8` to `u32` before summing to avoid overflow on realistic fan counts, then performs integer division by `fans.len() as u32` and casts back to `u8`. No rounding — truncating integer division. Called in `view()` to produce the panel fan% label; a `None` result renders as `"—"` (app.rs:124-127).

## get_popup SctkPopupSettings (src/app.rs:276-298)

The `TogglePopup` arm calls `self.core.applet.get_popup_settings(parent, new_id, None, None, None)` to build default settings, then immediately overrides `popup_settings.positioner.size_limits`:

```rust
popup_settings.positioner.size_limits = Limits::NONE
    .max_width(372.0)
    .min_width(300.0)
    .min_height(200.0)
    .max_height(1080.0);
```

All three `Option` arguments to `get_popup_settings` are `None` (letting the COSMIC runtime choose anchor, gravity, and offset). The `Limits` chain starts from `Limits::NONE` (unbounded) and then layers specific min/max bounds. After mutation, `get_popup(popup_settings)` (imported from `cosmic::iced::platform_specific::shell::wayland::commands::popup`) is returned as the `Task`.

## style() Trait Method (src/app.rs:310-312)

`AppModel` also implements the optional `style()` method of `cosmic::Application`:

```rust
fn style(&self) -> Option<cosmic::iced::theme::Style> {
    Some(cosmic::applet::style())
}
```

This hooks the COSMIC applet styling (transparent panel background, appropriate borders) into the iced theme system. Without this the applet would use the default iced window style.

## MAX_SAMPLES Ring-Buffer Cap (src/app.rs:22, 265-268)

`const MAX_SAMPLES: usize = 60` limits `temp_history: VecDeque<f64>` to 60 entries (approximately 90 seconds of history at the 1500 ms poll interval). The cap is enforced in `update()` with:

```rust
self.temp_history.push_back(status.liquid_temp_c);
while self.temp_history.len() > MAX_SAMPLES {
    self.temp_history.pop_front();
}
```

`VecDeque::pop_front` efficiently removes the oldest entry. The `while` loop (rather than `if`) is defensive against any future batch-insert path.
