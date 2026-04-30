# Coding Conventions

> Discovered coding conventions in the codebase.
> This file is append-only - agents add discoveries, never delete.

(Add conventions as you discover them)

## Naming Conventions

### Types
- Structs use `PascalCase`: `AppModel`, `AioStatus`, `Pump`, `Fan`, `Config`, `DeviceEntry`, `StatusEntry`
- Enums use `PascalCase` with descriptive variant names: `Message`, `Error`
- Error enum variants name their cause: `Spawn(io::Error)`, `Parse(serde_json::Error)`, `NonZeroExit { ... }`, `NoDevice`
- Message variants name the event or action: `TogglePopup`, `PopupClosed`, `UpdateConfig`, `StatusTick`

### Functions
- `snake_case` throughout: `fetch_status`, `parse_status_response`, `split_fan_key`, `view_window`
- Pure parsing functions are named `parse_<thing>_response` (liquidctl.rs:133)
- Helper functions are named `split_<thing>_key` for key parsing utilities (liquidctl.rs:204)
- Boolean-returning helpers use imperative names, not `is_` prefix where context is clear

### Variables
- `snake_case` for all locals: `liquid_temp_c`, `pump_speed`, `pump_duty`, `fan_list`
- Accumulator locals named after what they accumulate: `pump_speed`, `pump_duty`, `fans`
- Result variables named after their purpose, not their type: `label`, `content`, `column`
- Temp/intermediate: `raw` for raw string output (liquidctl.rs:122), `output` for Command output

### Modules
- `mod app` — applet model and Elm loop (app.rs)
- `mod config` — persisted configuration struct (config.rs)
- `mod i18n` — localization bootstrap (i18n.rs)
- `mod liquidctl` — subprocess integration (liquidctl.rs)
- All module files are `src/<module>.rs` (flat, no subdirectories)

### Fields
- Public struct fields use `snake_case` with units in name: `liquid_temp_c`, `speed_rpm`, `duty_pct`
- The `_c` / `_rpm` / `_pct` unit suffixes are the convention for sensor values
- Optional state fields use `Option<T>` named without `opt_` prefix: `popup`, `last_status`, `last_error`
- State history fields prefixed with `last_`: `last_status`, `last_error`

---

## Struct Organization Conventions

### AppModel field order (app.rs:16-27)
Runtime-managed fields first (`core`), then UI state (`popup`), then persisted state (`config`), then dynamic/polled state (`last_status`, `last_error`). Doc comments on every field.

### Public vs private structs
Public types that cross module boundaries derive `Debug` and `Clone` (liquidctl.rs:12-30). Private deserialization intermediaries derive only `Debug` and `Deserialize` (liquidctl.rs:88-104).

### Config struct (config.rs:5-9)
Derives `Debug`, `Default`, `Clone`, `CosmicConfigEntry`, `Eq`, `PartialEq`. Has `#[version = 1]` attribute. Placeholder fields use `String` type with name `demo`.

---

## License and File Header Conventions

Every source file begins with:
```text
// SPDX-License-Identifier: MPL-2.0
```
This is the first line before any imports or module-level doc comments.

---

## Comment Conventions

### Doc comments on structs and pub items
Public structs and their fields use `///` doc comments explaining purpose (app.rs:13-27). The same applies to `pub fn` in liquidctl.rs (liquidctl.rs:107, 131, 202).

### Module-level doc comment
Modules with a distinct purpose use `//\!` module doc at the top: `liquidctl.rs` and `i18n.rs` both have `//\!` module-level descriptions.

### In-line design rationale
Non-obvious decisions have inline comments: "Intentionally don't clear last_status — show stale data alongside the error." (app.rs:215). Commented-out tracing calls are left in place with the logging setup intact (app.rs:72-75, 189-191).

### Silenced warnings are explained
`#[allow(dead_code)]` is used on deserialization fields that are parsed but not used in the public API (`bus`, `address`, `unit`) (liquidctl.rs:89-91, 102).

---

## COSMIC Applet Trait Implementation Conventions

### Required associated types and const (app.rs:40-50)
```text
type Executor = cosmic::executor::Default;
type Flags = ();
type Message = Message;   // always the local Message enum
const APP_ID = "com.github.<user>.<AppName>"; // RDNN format
```

### Required methods
- `core(&self) -> &cosmic::Core` — returns `&self.core`
- `core_mut(&mut self) -> &mut cosmic::Core` — returns `&mut self.core`
- `init(core, _flags) -> (Self, Task<...>)` — constructs AppModel, returns `Task::none()`
- `view(&self) -> Element<...>` — panel button
- `view_window(&self, _id) -> Element<...>` — popup content
- `subscription(&self) -> Subscription<...>` — all background activity
- `update(&mut self, message) -> Task<...>` — message handler
- `style(&self) -> Option<...>` — returns `Some(cosmic::applet::style())`

### Optional lifecycle hook
`on_close_requested(&self, id: Id) -> Option<Message>` returns `Some(Message::PopupClosed(id))` to handle window manager close events (app.rs:86-88).

---

## Async and Import Conventions

### tokio usage
`tokio::process::Command` is used for subprocess execution (liquidctl.rs:8). `tokio::time::sleep` is used for poll delays inside iced stream channels (app.rs:180). No `#[tokio::main]` — iced/COSMIC owns the async runtime.

### futures imports
`futures_util::SinkExt` is imported for `.send().await` on `mpsc::Sender` (app.rs:10). `cosmic::iced::futures::channel::mpsc` is the channel type (app.rs:9).

### Prelude pattern
`use cosmic::prelude::*` is used to bring in the COSMIC widget trait methods and element types (app.rs:7).

---

## Testing Conventions (liquidctl.rs:213-267)

### Location
Tests are in a `#[cfg(test)] mod tests` block at the bottom of the file containing the function under test. Only `liquidctl.rs` has tests; no separate test files exist.

### Test function naming
`snake_case` describing the scenario: `parses_h150i_pro_xt_fixture`, `empty_array_yields_no_device`, `all_devices_empty_status_yields_no_device`.

### Fixture data
A `const FIXTURE: &str` at the top of the test module holds the full JSON string of a real `liquidctl --json status` response. Tests call `parse_status_response(FIXTURE)` directly, bypassing the subprocess.

### Assertion style
- Happy path: `.expect("fixture should parse")` then field-by-field `assert_eq\!` calls.
- Float comparison: `(value - expected).abs() < tolerance` with a custom failure message.
- Error path: `match result { Err(Error::NoDevice) => {}, other => panic\!("expected ..., got {other:?}") }` — matches the exact error variant, panics with the actual value otherwise.

### What is not tested
Subprocess invocation (`fetch_status`) is not tested — only the pure parsing function (`parse_status_response`) is exercised. No integration tests or mocking of `tokio::process::Command`.
