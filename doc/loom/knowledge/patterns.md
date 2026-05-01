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
- `config_handle: Option<cosmic_config::Config>` — raw handle kept alive for `write_entry`
- `pending_interval_secs: Option<f32>` — transient slider value during drag (`None` outside drag)
- `last_status: Option<AioStatus>` — last successful poll result
- `last_error: Option<String>` — last error string, coexists with `last_status`
- `temp_history: VecDeque<f64>` — liquid temp samples, capped at `HISTORY_CAP = 900`
- `pump_duty_history`, `fan_avg_duty_history: VecDeque<f64>` — duty histories rendered as 80 px popup sparklines (same cap; speed values are shown as numeric labels, not sparklines)

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

1. **liquidctl poll** (`app.rs:276-290`): `Subscription::run_with(interval_ms, fn_ptr)` wraps `cosmic::iced::stream::channel(4, ...)` — a channel with buffer size 4. The async closure runs an infinite loop: call `fetch_status`, send result, sleep `config.sample_interval_ms` (default 1500, user-configurable). If the send fails (channel closed), it breaks and then awaits `futures_util::future::pending()` to keep the future alive. The subscription identity includes `interval_ms`, so changing the configured interval tears down and restarts the poll loop.

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

`Sparkline` is a struct holding `samples: Vec<f64>` constructed from any `IntoIterator<Item = f64>` (sparkline.rs:18-27). It implements `canvas::Program<Message, Theme>` with `type State = ()` — no mutable canvas state (sparkline.rs:60-61).

The `draw` method (sparkline.rs:63-119):

- Returns early with an empty frame when `samples.is_empty()` (sparkline.rs:74-76).
- Computes the y-axis range via the pure helper `y_range(&self.samples)` (sparkline.rs:35-58), which auto-scales to the sample min/max but enforces a `MIN_Y_SPAN = 2.0` °C floor centered on the data midpoint (see "Sparkline y_range Helper" below).
- Computes a 1 px `pad` on each edge; `usable_w` and `usable_h` clamp to at least 1.0 via `.max(1.0)` (sparkline.rs:78-80).
- **Single-sample branch** (sparkline.rs:90-100): renders a horizontal tick at the sample's y across the full canvas width, so the sparkline is visible after the first poll instead of waiting for a second sample.
- **Multi-sample branch** (sparkline.rs:102-117): maps sample index `i` to x: `pad + (i / (n-1)) * usable_w`; maps value to y: `pad + (1 - norm) * usable_h`, where `norm = (sample - y_min) / range` and y is inverted so higher values render higher in the frame.
- Builds a `Path` via `Path::new(|p| { ... })` using `p.move_to` for the first point and `p.line_to` for subsequent points.
- Strokes the path with `Color::from_rgba8(180, 200, 230, 0.85)` (light blue, slightly transparent) at width 1.5.
- Returns `vec\![frame.into_geometry()]` — single geometry element per draw call.

The widget is embedded in `view()` as `Canvas::new(Sparkline::new(...))` with explicit `.width(Length::Fixed(36.0)).height(Length::Fixed(16.0))` (app.rs:129-131).

## Sparkline y_range Helper (src/sparkline.rs:35-58)

`fn y_range(samples: &[f64]) -> (f64, f64)` is a pure module-private helper that decouples the y-axis computation from the canvas draw call. This makes the scaling logic unit-testable without instantiating an iced renderer or canvas frame.

Algorithm:

1. Empty samples → return `(-1.0, 1.0)` (a `MIN_Y_SPAN`-wide band around 0). `draw` never invokes the helper for empty input, but the safe return prevents accidental division-by-zero if a future caller does.
2. Compute `min` and `max` across samples via a single forward pass (no allocations, no sort).
3. If `max - min < MIN_Y_SPAN`, expand symmetrically around the midpoint: `mid = (min + max) / 2`, return `(mid - MIN_Y_SPAN/2, mid + MIN_Y_SPAN/2)`.
4. Otherwise return `(min, max)` unchanged.

The `MIN_Y_SPAN = 2.0` °C floor is the load-bearing design decision: it suppresses sub-degree sensor noise (which would otherwise look like wild oscillations on a 2 °C-tall canvas) while letting any real spike of ≥ 2 °C fill the canvas vertically. The const is module-private; tests (sparkline.rs:122+) reference it via `use super::*`.

This pattern — extract a pure helper from a draw/render method specifically so the math can be tested in isolation — is the precedent for any future canvas widgets in the project.

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

## History Ring-Buffer Cap (src/app.rs:21-24, 55-59)

Originally a single constant capped only `temp_history` at 60 entries. This was replaced by two constants with distinct purposes:

```text
PANEL_SPARK_SAMPLES = 60    — how many trailing samples the panel button sparkline shows
HISTORY_CAP         = 900   — capacity of every per-metric VecDeque (~15 min at 1 s polling)
```

The cap is applied via the `push_capped` helper (`src/app.rs:55-59`) on each per-metric `VecDeque`:

```rust
fn push_capped(buf: &mut VecDeque<f64>, value: f64) {
    buf.push_back(value);
    while buf.len() > HISTORY_CAP {
        buf.pop_front();
    }
}
```

`VecDeque::pop_front` efficiently removes the oldest entry. The `while` loop (rather than `if`) is defensive against any future batch-insert path. The panel sparkline receives only the trailing `PANEL_SPARK_SAMPLES` entries via `.skip(len.saturating_sub(PANEL_SPARK_SAMPLES))`.

## Canvas Gradient-Filled Sparkline (src/sparkline.rs)

The sparkline draws a gradient-filled area under the polyline so trends are easier to read at a glance. Three structural rules govern this:

**Area path construction.** The area is a single closed polygon formed by tracing the baseline-left corner, then all polyline points in order, then the baseline-right corner, then `close()`. The `close()` call ties the bottom-right corner back to the bottom-left implicitly. For the multi-sample branch (`src/sparkline.rs:157-164`):

```text
move_to(pad,            pad + usable_h)    // baseline, left edge
line_to(x0,             y0)               // up to first sample
line_to(x1,             y1)
…
line_to(x_{n-1},        y_{n-1})          // last sample
line_to(pad + usable_w, pad + usable_h)   // drop to baseline at right edge
close()                                   // close back to baseline-left
```

**Gradient API.** The libcosmic-pinned `canvas::gradient::Linear` takes start/end `Point` values — NOT an angle. Top-to-bottom in screen coordinates is `start = Point::new(0.0, 0.0)` to `end = Point::new(0.0, bounds.height)` (`src/sparkline.rs:71-80`). The angle-based `Linear` in `iced_core::gradient` is for background fills and is a different type; do not confuse them. The `area_gradient` helper (`src/sparkline.rs:71`) builds the gradient: top stop alpha 0.55, bottom stop alpha 0.0.

**Draw order.** `frame.fill(&area, gradient)` is called BEFORE `frame.stroke(&polyline, stroke)` (`src/sparkline.rs:177-178`) so the polyline sits on top of the gradient and remains fully visible.

**Single-sample branch** (`src/sparkline.rs:120-139`): builds a closed rectangle from the tick y-coordinate down to the baseline, fills it with the gradient, then strokes the horizontal tick on top. Same fill-before-stroke order applies.

## Theme-Accent Color in Canvas Programs (src/sparkline.rs:106-112)

Inside `canvas::Program::draw`, the active COSMIC accent is available via:

```rust
let srgba = theme.cosmic().accent.base;
```

The type of `accent.base` is `palette::Srgba`, which is an alias for `Alpha<Rgb<Srgb, f32>, f32>`. Field access: `.red`, `.green`, `.blue` are exposed via `Deref` on the inner `Rgb`; `.alpha` is on the outer `Alpha` wrapper.

There is no `From<Srgba> for iced::Color`, so the color must be constructed manually:

```rust
let accent = Color {
    r: srgba.red,
    g: srgba.green,
    b: srgba.blue,
    a: self.stroke_alpha,
};
```

The `theme` parameter is slot 3 of `draw` (previously `_theme: &Theme` — the leading underscore was removed when the parameter became used).

## Subscription Restart on Config Change (src/app.rs:269-302)

`Subscription::run_with(data, fn_ptr)` takes a `fn` pointer, not a closure with captures. Iced's subscription identity is `(data, fn-ptr)`: when `data` changes, the old stream tears down and a new one starts. This is the canonical iced/cosmic idiom for re-keying a background stream when a config value changes.

To pass a configurable value into the stream:

1. Place the value in `data` (e.g. `interval_ms: u64`).
2. Inside the non-capturing `fn`, dereference `data` with `*` to get an owned copy, then `move` that copy into the inner `async` block.

```rust
let interval_ms = self.config.sample_interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);

let liquidctl_sub = Subscription::run_with(interval_ms, |interval_ms: &u64| {
    let interval_ms = *interval_ms;  // copy out of the reference — fn can't capture
    cosmic::iced::stream::channel(4, move |mut channel: mpsc::Sender<Message>| async move {
        loop {
            // … fetch, send, sleep interval_ms …
        }
    })
});
```

Because only `SampleIntervalReleased` mutates `self.config.sample_interval_ms`, the subscription identity — and therefore the running poll loop — is stable during a drag. The loop only tears down and restarts after the user releases the slider and commits a new value (`src/app.rs:269-302`).

## Subscription Composite Key (`(interval_ms, match_str)`) and Optional Poll (src/app.rs)

`Subscription::run_with` requires only `D: Hash + 'static`, not `Send + Sync + Clone`. A tuple of `(u64, String)` qualifies, so the device-selector subscription threads the match string through alongside the interval:

```rust
let key: (u64, String) = (interval_ms, match_str);
Subscription::run_with(key, |key: &(u64, String)| {
    let interval_ms = key.0;
    let match_str = key.1.clone();
    cosmic::iced::stream::channel(4, move |mut channel| async move {
        loop { /* fetch_status(&match_str) … sleep interval_ms */ }
    })
})
```

The fn pointer is non-capturing (the data tuple rides through), and the inner `async move` clones the string out of the borrow on each subscription start. Either dimension changing — committed interval OR selected device — re-keys the subscription identity and restarts the loop.

**Optional poll subscription pattern.** `subscription()` builds a `Vec<Subscription<Message>>` and only appends the poll when `effective_match()` is `Some`. Until the first `DevicesEnumerated` Task lands, no poll runs at all — this prevents the panel from flashing a spurious "no AIO device" error during the brief startup window before enumeration completes. The config-watch sub is always installed and is the only sub before enumeration finishes.

## AIO Auto-Detection via Substring Catalog (src/devices.rs)

Device classification is split between three layers so the policy can evolve without touching the parser or the UI:

1. **`AIO_PATTERNS: &[&str]`** (`src/devices.rs`) — module-private array of lowercase substrings. Each entry is intentionally narrow (e.g. `"hydro"`, `"icue h"`) and corresponds to a liquidctl driver family verified against the current `parse_status_response` schema.
2. **`is_aio(description)`** — single `to_ascii_lowercase` on the description, then `AIO_PATTERNS.iter().any(|p| d.contains(p))`. Patterns are pre-lowercased so the per-call hot path allocates the description once and does no allocation per pattern.
3. **`filter_aios` / `auto_select`** — pure functions returning borrowed slices/refs from the caller-owned `&[DetectedDevice]`. No ownership transfer, no clones.

The actual `--match` value sent to liquidctl is the device's full `description` (verbatim), not a pattern. Patterns classify; descriptions select. liquidctl performs case-insensitive substring matching on the description, so a unique device of any given family is uniquely selected. With two identical AIOs connected, this is ambiguous (liquidctl picks the first); disambiguation via `--bus`/`--address` is deferred — `DetectedDevice` already carries `bus` and `address` for the eventual follow-up.

Adding a new family means adding one lowercase pattern AND verifying the parser handles that driver's status schema (key names, `Coolant temperature` vs `Liquid temperature`, indexed vs non-indexed `Fan N`). The current catalog is restricted to `hydro_platinum.py`-derived devices; `Compatibility Constraints` in `doc/plans/PLAN-device-selector.md` enumerates deferred families.

## Subprocess Serialization via tokio::sync::Mutex (src/liquidctl.rs)

Every `liquidctl` subprocess call (`fetch_status`, `list_devices`) acquires a module-private async mutex at the top of the function:

```rust
static LIQUIDCTL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub async fn fetch_status(match_filter: &str) -> Result<AioStatus, Error> {
    let _guard = LIQUIDCTL_LOCK.lock().await;
    // … existing body …
}
```

The motivation is the exclusive `O_RDWR` claim that `liquidctl` opens on `/dev/hidrawN` per device via HIDAPI. If a popup-open enumerate fires while a status poll is mid-flight on the same device, the second invocation can fail with a permission/busy error, surfacing as a spurious `last_error`. Holding the mutex guarantees only one liquidctl subprocess runs at a time. The lock is held only for the subprocess duration (≤3 s for status, ≤1 s for list); contention is bounded to one poll-cycle in the worst case.

This is `tokio::sync::Mutex` (held across `.await`), NOT `std::sync::Mutex`. The static initializer is `LazyLock::new(|| Mutex::new(()))` — `tokio::sync::Mutex::new` is not `const`-constructible.

## Tolerant Field Deserialization with serde_json::Value (src/liquidctl.rs)

`liquidctl list --json` typically emits `bus` and `address` as strings, but defensive deserialization through `serde_json::Value` protects against future driver changes that might emit numbers (e.g. numeric USB addresses on some backends):

```rust
fn deserialize_string_lossy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

#[derive(Deserialize)]
struct ListDeviceEntry {
    description: String,
    #[serde(deserialize_with = "deserialize_string_lossy")]
    bus: String,
    #[serde(deserialize_with = "deserialize_string_lossy")]
    address: String,
}
```

This is the precedent for any future field whose JSON type is not strictly contractual. Note that `description` is NOT made tolerant — it is structurally required to be a string and a non-string description is a real error worth surfacing. Use the lossy treatment only on truly metadata-shaped fields where empty-on-mismatch is preferable to parse failure.

## Effective-Value Resolution Helper (src/app.rs::effective_match)

When configuration carries `Option<T>` semantics (None = "auto"), encapsulate the auto-vs-explicit resolution behind a single private method:

```rust
fn effective_match(&self) -> Option<String> {
    self.config.device_match.clone()
        .or_else(|| devices::auto_select(&self.detected_devices)
            .map(|d| d.description.clone()))
}
```

Every call site (subscription key, dropdown selection, history-reset diff) uses this exact method instead of inlining the auto-select logic. The benefits:

- The "did the effective device change?" diff in `DevicesEnumerated`/`DeviceSelected` is just `prev != new` against snapshots taken before/after the mutation.
- A semantic no-op (e.g. user explicitly picks the device that auto-detect would have picked anyway) skips the history reset, because `effective_match()` returned the same value before and after.
- Adding a future "effective sample rate" or similar Option-config-with-fallback follows the same pattern.

## reset_device_state Coalescing Helper (src/app.rs)

When a model-state mutation invalidates multiple correlated buffers, extract the clearing into one private helper rather than duplicating it at each call site:

```rust
fn reset_device_state(&mut self) {
    self.temp_history.clear();
    self.pump_duty_history.clear();
    self.fan_avg_duty_history.clear();
    self.last_status = None;
    self.last_error = None;
}
```

Both `DevicesEnumerated` (auto-pick changed) and `DeviceSelected` (explicit pick changed) invoke this. New per-device state added in the future (e.g. another sparkline history) is reset by editing one place. The helper is `&mut self`-only and does not return a `Task`, so it composes cleanly inside an `update` arm that may also need to return a Task downstream.
