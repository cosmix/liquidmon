# Architectural Patterns

> Discovered patterns in the codebase that help agents understand how things work.
> This file is append-only - agents add discoveries, never delete.

(Add patterns as you discover them)

## Error Handling Patterns

### Domain error enum with Display and std::error::Error (liquidctl.rs:33-85)

`liquidctl.rs` defines a custom `Error` enum with four variants covering all failure modes of the liquidctl CLI integration:

```text
Error::Spawn(io::Error)          — process could not be spawned
Error::NonZeroExit { status, stderr } — process ran but failed
Error::Parse(serde_json::Error)  — stdout was valid UTF-8 but bad JSON
Error::NoDevice                  — JSON parsed but no usable device found
```

`Display` is hand-implemented with match arms that produce human-readable messages (liquidctl.rs:43-63). `std::error::Error::source()` is implemented to chain underlying `io::Error` or `serde_json::Error` causes (liquidctl.rs:65-73). `From<io::Error>` and `From<serde_json::Error>` allow `?` propagation inside `fetch_status` (liquidctl.rs:75-85).

### ? propagation with map_err for type conversion (liquidctl.rs:108-128)

`fetch_status` uses `.map_err(Error::Spawn)?` for the spawn step and an explicit closure to wrap a UTF-8 decode error as `Error::Spawn(io::Error::new(...))` (liquidctl.rs:113, 122-127). The serde_json parse step uses plain `?` via the `From` impl.

### Lossy error conversion at the app boundary (app.rs:35, 176-177)

At the Elm message boundary, `Result<AioStatus, Error>` is converted to `Result<AioStatus, String>` via `.map_err(|e| format\!("{e}"))` before being wrapped in `Message::StatusTick` (app.rs:176). This erases error structure but keeps a human-readable description for display.

### Stale-data preservation on error (app.rs:212-215)

In `update`, the `StatusTick(Err(msg))` arm sets `last_error` but intentionally does NOT clear `last_status` — an explicit design decision documented with an in-code comment. This means the popup continues showing the last good reading alongside the error message.

### Config load with error degradation (app.rs:68-79)

Config loading uses a nested `map`/`match` chain: `cosmic_config::Config::new(...)` maps over a successful context, tries `Config::get_entry`, and on `Err((_errors, config))` discards the errors and falls back to the partial config. Then `.unwrap_or_default()` handles the case where no config context exists at all. Error logging is commented out but present as dead code.

---

## State Management Patterns

### Single flat AppModel struct (app.rs:16-27)

All application state lives in one struct with five fields:

- `core: cosmic::Core` — runtime-managed, accessed via required trait methods
- `popup: Option<Id>` — popup window identity, `None` when closed
- `config: Config` — persisted config reloaded via subscription
- `last_status: Option<AioStatus>` — last successful poll result
- `last_error: Option<String>` — last error string, coexists with `last_status`

`#[derive(Default)]` is present so `init` can use struct update syntax (`..Default::default()`).

### Explicit Option states for status/error (app.rs:116-155)

`view_window` uses a three-way match on `(&self.last_status, &self.last_error)`:

- `(Some(status), maybe_err)` — display data, optionally annotate with error caption
- `(None, Some(err))` — full error state, no data yet
- `(None, None)` — initial loading state, show placeholder text

This pattern encodes "stale + error" distinctly from "no data + error".

### Toggle idiom for popup state (app.rs:218-236)

`Message::TogglePopup` uses `self.popup.take()` to atomically remove the current id — if `Some`, destroy the popup; if `None`, create a new one with `Id::unique()` and store it. The early `return` exits `update` with the task.

---

## Async Patterns

### Subscription-based background polling (app.rs:166-196)

The subscription uses `Subscription::batch` containing two subscriptions:

1. **liquidctl poll** (`app.rs:169-185`): `Subscription::run_with("liquidctl-sub", ...)` wraps `cosmic::iced::stream::channel(4, ...)` — a channel with buffer size 4. The async closure runs an infinite loop: call `fetch_status`, send result, sleep 1500 ms. If the send fails (channel closed), it breaks and then awaits `futures_util::future::pending()` to keep the future alive.

2. **Config watch** (`app.rs:187-195`): `self.core().watch_config::<Config>(APP_ID).map(...)` maps config update events to `Message::UpdateConfig`.

### tokio::process::Command for subprocess (liquidctl.rs:109-119)

`tokio::process::Command::new("liquidctl")` is used with `.output().await` — this captures both stdout and stderr without streaming. The async function is called from within the iced stream channel closure using `await`.

### Channel and SinkExt for message production (app.rs:172, 177-178)

The stream closure receives `mpsc::Sender<Message>` and calls `channel.send(...).await` from `futures_util::SinkExt`. The `.is_err()` check on send is the only way to detect that the applet is shutting down.

---

## UI Construction Patterns

### Panel button view using core.applet.text (app.rs:95-110)

`view()` produces a single `widget::button::custom(text_widget)` using `self.core.applet.text(label)` for the label. The button class is `cosmic::theme::Button::AppletIcon`. The label string has three states: temperature string, "\!" for error, "…" for loading.

### Popup content via list_column builder (app.rs:118-155)

`view_window` builds popup content using `widget::list_column()` with chained `.add(...)` calls. The column is built imperatively:

- Always adds a heading row from `status.description`
- Then fixed rows for liquid temp and pump
- Then a dynamic loop over `status.fans` adding one row per fan
- Then an optional error caption if `maybe_err.is_some()`

The column is converted to an `Element` via `.into()` at each match arm exit. The popup is wrapped with `self.core.applet.popup_container(content).into()`.

### Text hierarchy helpers (app.rs:120-145)

Three text widget helpers are used for typographic hierarchy:

- `widget::text::heading(...)` — device description
- `widget::text::body(...)` — sensor readings
- `widget::text::caption(...)` — error annotation

---

## Message/Command (Elm-like) Patterns

### Message enum with four variants (app.rs:31-36)

```text
Message::TogglePopup           — UI interaction (no data)
Message::PopupClosed(Id)       — window manager event
Message::UpdateConfig(Config)  — config subscription event
Message::StatusTick(Result<AioStatus, String>) — poll result
```

All variants derive `Debug` and `Clone`. The `Result` inside `StatusTick` is already stringified.

### update returns Task::none() except for popup toggle (app.rs:204-245)

All message arms except `TogglePopup` end with `Task::none()`. Only `TogglePopup` returns an actual task (`destroy_popup` or `get_popup`), using an early `return` to short-circuit the trailing `Task::none()`.

### init returns Task::none() (app.rs:83)

`init` does no async work; the subscription drives all background activity.

---

## Parsing Patterns (liquidctl.rs)

### Two-phase JSON parsing (liquidctl.rs:133-198)

Phase 1: deserialize to `Vec<DeviceEntry>` (private structs). Phase 2: iterate `device.status` and extract fields by `entry.key` string matching into local `Option` accumulators. Unknown keys are silently ignored (liquidctl.rs:169). Fan entries are accumulated into a `BTreeMap<u8, (Option<u32>, Option<u8>)>` keyed by fan index for natural ordering.

### Private deserialization structs separate from public API types (liquidctl.rs:88-104)

`DeviceEntry` and `StatusEntry` are private structs used only for deserialization. Public types `AioStatus`, `Pump`, `Fan` are the parsed, typed representation. Fields unused in parsing are annotated `#[allow(dead_code)]`.

### BTreeMap for ordered fan accumulation (liquidctl.rs:145-188)

`BTreeMap` is chosen over `HashMap` so fans are naturally ordered by index during `.into_iter()`, avoiding an explicit sort (though an explicit `.sort_by_key` is still called as belt-and-suspenders at liquidctl.rs:188).
