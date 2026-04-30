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
