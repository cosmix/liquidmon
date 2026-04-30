# Coding Conventions

> Discovered coding conventions in the codebase.
> This file is append-only - agents add discoveries, never delete.

## Naming Conventions

### Types

- Structs use `PascalCase`: `AppModel`, `AioStatus`, `Pump`, `Fan`, `Config`, `DeviceEntry`, `StatusEntry`, `Sparkline`
- Enums use `PascalCase` with descriptive variant names: `Message`, `Error`
- Error enum variants name their cause: `Spawn(io::Error)`, `Parse(serde_json::Error)`, `NonZeroExit { ... }`, `NoDevice`, `MissingField(&'static str)`, `Timeout`
- `MissingField` carries a `&'static str` field name (e.g. `"liquid temperature"`) — not a `String` — to avoid allocation and allow direct pattern matching in tests
- Message variants name the event or action: `TogglePopup`, `PopupClosed`, `UpdateConfig`, `StatusTick`

### Functions

- `snake_case` throughout: `fetch_status`, `parse_status_response`, `split_fan_key`, `view_window`, `symbolic_icon`, `fan_duty_avg`
- Pure parsing functions are named `parse_<thing>_response` (liquidctl.rs:142)
- Helper functions are named `split_<thing>_key` for key parsing utilities (liquidctl.rs:217)
- Boolean-returning helpers use imperative names, not `is_` prefix where context is clear

### Variables

- `snake_case` for all locals: `liquid_temp_c`, `pump_speed`, `pump_duty`, `fan_list`
- Accumulator locals named after what they accumulate: `pump_speed`, `pump_duty`, `fans`
- Result variables named after their purpose, not their type: `label`, `content`, `column`
- Temp/intermediate: `raw` for raw string output (liquidctl.rs:131), `output` for Command output

### Modules

- `mod app` — applet model and Elm loop (app.rs)
- `mod config` — persisted configuration struct (config.rs)
- `mod liquidctl` — subprocess integration (liquidctl.rs)
- `mod sparkline` — temperature sparkline canvas widget (sparkline.rs)
- All module files are `src/<module>.rs` (flat, no subdirectories)

### Fields

- Public struct fields use `snake_case` with units in name: `liquid_temp_c`, `speed_rpm`, `duty_pct`
- The `_c` / `_rpm` / `_pct` unit suffixes are the convention for sensor values
- Optional state fields use `Option<T>` named without `opt_` prefix: `popup`, `last_status`, `last_error`
- State history fields prefixed with `last_`: `last_status`, `last_error`
- Ring-buffer history fields named descriptively: `temp_history: VecDeque<f64>`

---

## Struct Organization Conventions

### AppModel field order (app.rs:44-58)

Runtime-managed fields first (`core`), then UI state (`popup`), then persisted state (`config`), then dynamic/polled state (`last_status`, `last_error`, `temp_history`). Doc comments on every field.

### Public vs private structs

Public types that cross module boundaries derive `Debug` and `Clone` (liquidctl.rs:12-31). Private deserialization intermediaries derive only `Debug` and `Deserialize` (liquidctl.rs:95-111).

### Config struct (config.rs:5-7)

Derives `Debug`, `Default`, `Clone`, `CosmicConfigEntry`, `Eq`, `PartialEq`. Has `#[version = 1]` attribute. Currently an empty braced struct (`pub struct Config {}`); `CosmicConfigEntry` derive accepts empty structs.

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

Public structs and their fields use `///` doc comments explaining purpose (app.rs:42-58). The same applies to `pub fn` in liquidctl.rs (liquidctl.rs:113, 140, 214).

### Module-level doc comment

Modules with a distinct purpose use `//!` module doc at the top: `liquidctl.rs` has a `//!` module-level description.

### In-line design rationale

Non-obvious decisions have inline comments: "Intentionally don't clear last_status — show stale data alongside the error." (app.rs:268). Bounded cast helpers are annotated with their domain at the definition site (liquidctl.rs:150).

### Silenced warnings are explained

`#[allow(dead_code)]` is used on deserialization fields that are parsed but not used in the public API (`bus`, `address`, `unit`) (liquidctl.rs:97-99, 109).

---

## COSMIC Applet Trait Implementation Conventions

### Required associated types and const (app.rs:71-81)

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

`on_close_requested(&self, id: Id) -> Option<Message>` returns `Some(Message::PopupClosed(id))` to handle window manager close events (app.rs:111-113).

---

## Async and Import Conventions

### tokio usage

`tokio::process::Command` is used for subprocess execution (liquidctl.rs:9). `tokio::time::sleep` is used for poll delays inside iced stream channels (app.rs:235). `tokio::time::timeout` wraps the subprocess `.output()` call with a 3 s deadline (liquidctl.rs:119). No `#[tokio::main]` — iced/COSMIC owns the async runtime.

### futures imports

`futures_util::SinkExt` is imported for `.send().await` on `mpsc::Sender` (app.rs:14). `cosmic::iced::futures::channel::mpsc` is the channel type (app.rs:6).

### Prelude pattern

`use cosmic::prelude::*` is used to bring in the COSMIC widget trait methods and element types (app.rs:11).

---

## Testing Conventions

### Location

Tests are in a `#[cfg(test)] mod tests` block at the bottom of the file containing the function under test. Currently `src/liquidctl.rs` (parser) and `src/app.rs` (helpers + `update` state transitions). No separate test files exist.

### Test function naming

`snake_case` describing the scenario, no `test_` prefix. Names use an outcome-shaped phrase: `parses_h150i_pro_xt_fixture`, `empty_array_yields_no_device`, `device_missing_liquid_temp_yields_missing_field`, `fan_with_only_speed_is_dropped`, `out_of_range_pump_duty_is_clamped`, `status_tick_err_preserves_stale_status`, `temp_history_caps_at_max_samples`.

### Fixture data

A `const FIXTURE: &str` at the top of `liquidctl::tests` holds the full JSON string of a real `liquidctl --json status` response. Tests call `parse_status_response(FIXTURE)` directly, bypassing the subprocess. Smaller scenario-specific JSON literals are inlined into individual tests as raw strings.

### `app.rs` test setup

`app::tests` imports `cosmic::Application as _` to bring the trait method `update` into scope, then constructs the model with `AppModel::default()`. Test-only fixture builders are declared inside the test module (`fan(index, duty_pct)`, `sample_status(temp_c)`) to keep individual tests short. State transitions are exercised by calling `model.update(Message::...)` and then asserting on `model`'s public fields.

### Assertion style

- Happy path: `.expect("fixture should parse")` then field-by-field `assert_eq!` calls.
- Float comparison: `(value - expected).abs() < tolerance` with a custom failure message (or a small `1e-9` tolerance for values written as exact f64 literals).
- Error path: `match result { Err(Error::NoDevice) => {}, other => panic!("expected ..., got {other:?}") }` — matches the exact error variant, panics with the actual value otherwise. For `MissingField`, the inner string literal is also matched: `Err(Error::MissingField("liquid temperature")) => {}`.

### What is not tested

- Subprocess invocation (`fetch_status`) is not tested — only the pure parsing function (`parse_status_response`) is exercised. No integration tests or mocking of `tokio::process::Command`.
- `view` / `view_window` rendering, the `subscription` background task, and the `TogglePopup` arm of `update` (which depends on `core.main_window_id()`).

## Import Organization

Imports in each file follow this order (app.rs:3-17 is the clearest example):

1. Local crate modules (`use crate::...`) — `crate::config::Config`, `crate::sparkline::Sparkline`
2. Third-party crates in dependency order — `cosmic::...` items grouped together, then `futures_util`, then `std`
3. `std` imports last — `std::collections::VecDeque`, `std::sync::LazyLock`, `std::time::Duration`

Within a third-party crate, submodules are imported in separate `use` lines rather than nested braces. There are no blank lines separating import groups — all `use` statements are contiguous.

In `liquidctl.rs` (lines 5-9), the order is: `serde`, then `std` modules, then `tokio`. This is less strict than app.rs; the convention is "external before std is acceptable" but `std` before local is consistent.

## Derive Attribute Ordering

Public domain structs derive in this order: `Debug, Clone` (liquidctl.rs:12, 20, 26). The `Config` struct derives `Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq` (config.rs:5) — standard traits first, then derive-macro traits, then equality traits. `AppModel` derives only `Default` (app.rs:44). `Message` derives `Debug, Clone` (app.rs:61). Private serde structs derive `Debug, Deserialize` (liquidctl.rs:95, 105). The pattern is: `Debug` always first, `Clone` second when present, then domain-specific derives (`Deserialize`, `CosmicConfigEntry`), then equality derives last.

## Visibility Conventions

- `pub struct` for types that cross module boundaries: `AppModel`, `AioStatus`, `Pump`, `Fan`, `Config`, `Sparkline`
- `pub` fields on domain data structs: all fields of `AioStatus`, `Pump`, `Fan` are `pub`
- Private structs for internal implementation details: `DeviceEntry`, `StatusEntry` (no `pub`)
- Private fields on `AppModel`: all six fields are private (no `pub`)
- `pub async fn` for the public API function: `fetch_status` (liquidctl.rs:115)
- Private `fn` for implementation helpers: `parse_status_response`, `split_fan_key`, `symbolic_icon`, `fan_duty_avg`
- No `pub(crate)` is used anywhere in the codebase — the choice is strictly `pub` or private.

## Clippy and Lint Configuration

No `#\![allow(...)]` or `#\![deny(...)]` attributes at the crate level (main.rs has none). No `#[clippy::...]` attributes on functions. The only lint suppression is item-level `#[allow(dead_code)]` on specific struct fields that are parsed but unused: `DeviceEntry.bus`, `DeviceEntry.address`, `StatusEntry.unit` (liquidctl.rs:97-99, 109). This is the narrowest possible suppression scope — annotate the field, not the struct or module.

## Monomorphic font() Call Convention

Numeric text in the popup is always styled with `.font(cosmic::font::mono())` chained after the text widget (app.rs:179, 184, 193). Non-numeric text (headings, error messages) uses the default font. This distinguishes sensor readings visually in the popup without requiring separate widget types.
