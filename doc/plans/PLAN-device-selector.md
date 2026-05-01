# PLAN: Device Selector for AIO Cooler

## Context

Today the panel applet hard-codes the liquidctl `--match` string to `"Hydro"` at `src/app.rs:280`. This silently excludes the majority of liquidctl-supported AIOs (NZXT Kraken, Corsair iCUE, MSI Coreliquid, ASUS Ryujin, EVGA CLC, Aquacomputer D5 Next, Lian Li Galahad II LCD, etc.) — anyone whose cooler doesn't have "Hydro" in its description sees only an error badge.

We will:

1. Enumerate connected liquidctl devices (`liquidctl list --json`).
2. Classify them against an internal catalog of substrings that cover the vast majority of supported AIOs.
3. Auto-pick the first AIO match for the poll subscription.
4. Surface the detected AIOs in a dropdown beneath the popup slider so the user can override the auto-pick. Selection is persisted via `cosmic-config` and always honoured when present.

`liquidctl --match` is **case-insensitive substring matching** against the device description (`liquidctl/driver/usb.py::probe`: `if match.lower() not in desc.lower(): continue`). The dropdown stores the **full device description** as the match value. With one cooler of any given family connected, this selects a unique device. With two identical coolers (e.g. dual H150i Pro XT in a workstation), `--match` substring matching is ambiguous and liquidctl picks the first — see "Known Limitations" below; truly-unique selection via `--bus`/`--address` is deferred to a follow-up plan.

## Compatibility Constraints

The current `parse_status_response` (`src/liquidctl.rs:131-201`) requires four key shapes from the liquidctl JSON status output:

- `"Liquid temperature"` (exact key match)
- `"Pump speed"` and `"Pump duty"` (both required — `MissingField` if absent)
- `"Fan N speed"` AND `"Fan N duty"` for each fan (fans missing one are silently dropped at `liquidctl.rs:181-188`)

Per liquidctl driver-source review:

| Driver              | Devices                                  | Schema match                                                                                                                                 | v1 status |
| ------------------- | ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | --------- |
| `hydro_platinum.py` | Corsair Hydro Pro / Pro XT / Platinum    | All 4 keys present                                                                                                                           | ✅ ship   |
| `hydro_platinum.py` | Corsair iCUE Elite (Capellix / RGB)      | All 4 keys present                                                                                                                           | ✅ ship   |
| `kraken3.py`        | NZXT Kraken X3 / Z3 / 2023 / 2024        | Uses `"Fan speed"` / `"Pump speed"` etc. — **schema differs** (no `Fan N` indexing for single-fan models; key names not verified end-to-end) | ⏸ defer   |
| `asetek.py`         | Legacy NZXT Kraken X (X41/X61), EVGA CLC | Reports temp + pump + fan speeds; pump/fan duty support is per-firmware and may emit no duty key                                             | ⏸ defer   |
| `asetek_pro.py`     | Corsair Hydro Pro (non-XT)               | Likely compatible but unverified                                                                                                             | ⏸ defer   |
| `coolit.py`         | Corsair H110i GT                         | Unverified                                                                                                                                   | ⏸ defer   |
| `aquacomputer.py`   | Aquacomputer D5 Next                     | Uses `"Coolant temperature"` — **does not match** parser                                                                                     | ⏸ defer   |
| `msi.py`            | MSI MPG Coreliquid K360                  | Unverified                                                                                                                                   | ⏸ defer   |
| `asus_ryujin.py`    | ASUS Ryujin II / III                     | Unverified                                                                                                                                   | ⏸ defer   |
| `ga2_lcd.py`        | Lian Li Galahad II LCD                   | Unverified                                                                                                                                   | ⏸ defer   |

**v1 ships the verified-compatible families only.** Including unverified families would surface as `MissingField` errors when the user picks them — worse UX than not offering them at all. A follow-up plan (`PLAN-aio-broad-support.md`, not part of this PR) will:

1. Make `Pump.duty_pct` and `Fan.duty_pct` `Option<u8>`.
2. Recognize `"Coolant temperature"` as a synonym of `"Liquid temperature"`.
3. Handle non-indexed `"Fan speed"` / `"Fan duty"` keys for single-fan devices.
4. Skip the corresponding metric histories when a value is `None`.
5. Broaden `AIO_PATTERNS` to cover the deferred families.

This v1/v2 split keeps the device-selector PR focused on UI + selection plumbing, and isolates the parser changes (which touch every history push site and the popup view) into their own change.

## AIO Substring Catalog

A new module-private constant in `src/devices.rs`. v1 ships two patterns (hydro_platinum.py family); broader liquidctl coverage is deferred per the Compatibility Constraints section above:

```rust
// Patterns are written lowercase so `is_aio` can do a single
// `to_ascii_lowercase` on the description and skip per-call
// allocation on each pattern. Restricted to families verified
// against the current parser schema (see Compatibility Constraints).
const AIO_PATTERNS: &[&str] = &[
    "hydro",   // Corsair Hydro Pro / Pro XT / Platinum (hydro_platinum.py)
    "icue h",  // Corsair iCUE Elite Capellix / RGB     (hydro_platinum.py)
];
```

Patterns are intentionally narrow strings used only to **classify** what's a parser-compatible AIO; the actual `--match` value sent to liquidctl is the device's full description, not a pattern.

## Files Modified / Added

| File                         | Change                                                                                                                     |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `src/config.rs`              | Add `device_match: Option<String>` field; bump `#[version = 3]`; update hand `Default`                                     |
| `src/liquidctl.rs`           | Add `pub async fn list_devices() -> Result<Vec<DetectedDevice>, Error>`, `DetectedDevice`, `LIQUIDCTL_LOCK` mutex, + tests |
| `src/devices.rs` NEW         | `AIO_PATTERNS`, `is_aio(desc)`, `filter_aios`, `auto_select(devices)`, plus unit tests                                     |
| `src/main.rs`                | Add `mod devices;`                                                                                                         |
| `src/app.rs`                 | New messages, new model fields, init enumerate task, dropdown UI, subscription re-keying, `reset_device_state` helper      |
| `Cargo.toml`                 | Update `description` and extended `[package.metadata.deb].extended-description`                                            |
| `resources/app.desktop`      | Update `Comment=`                                                                                                          |
| `resources/app.metainfo.xml` | Update `<summary>`                                                                                                         |
| `README.md`                  | Update opening paragraph, Supported Devices section, and example `liquidctl` command                                       |

## Detailed Design

### 1. `src/config.rs`

```rust
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 3]
pub struct Config {
    pub sample_interval_ms: u64,
    /// User-selected liquidctl device description (verbatim, used as
    /// `--match` substring). `None` means auto-detect at runtime.
    pub device_match: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1500,
            device_match: None,
        }
    }
}
```

`CosmicConfigEntry`'s field-by-field fallback handles v2→v3 upgrade — existing users keep their interval and get `device_match: None`.

### 2. `src/liquidctl.rs` — device enumeration

Add a new public function plus a small public type (kept narrow — UI never sees raw `DeviceEntry`):

```rust
#[derive(Debug, Clone)]
pub struct DetectedDevice {
    pub description: String,
    pub bus: String,
    pub address: String,
}

pub async fn list_devices() -> Result<Vec<DetectedDevice>, Error> {
    // liquidctl list --json   (no --match: enumerate everything)
    // kill_on_drop + 1 s timeout — `list` is purely an HID enumeration,
    // it does not transact with the device, so 3 s is much too generous.
}

fn parse_devices_response(raw: &str) -> Result<Vec<DetectedDevice>, Error> {
    // Returns Ok(vec![]) on `[]` (empty is a valid state, NOT NoDevice).
}
```

**Schema note (verified on this machine):** `liquidctl list --json` emits objects with `description`, `vendor_id`, `product_id`, `release_number`, `serial_number`, `bus`, `address`, `port`, `driver`, `experimental` — **no `status` field**. The existing private `DeviceEntry` struct requires `status: Vec<StatusEntry>` and would fail to deserialize the `list` payload with `Error::Parse` on every call. Therefore `parse_devices_response` deserializes into a **separate** private struct:

```rust
#[derive(Debug, Deserialize)]
struct ListDeviceEntry {
    description: String,
    /// Tolerant: liquidctl normally emits a string (`"hid"`, `"usb"`),
    /// but defensive deserialization through serde_json::Value protects
    /// against future driver changes that might emit null. Empty string
    /// on non-string input.
    #[serde(deserialize_with = "deserialize_string_lossy")]
    bus: String,
    /// Same defensive treatment — USB addresses can in principle be
    /// numeric tuples in some liquidctl backends.
    #[serde(deserialize_with = "deserialize_string_lossy")]
    address: String,
}

fn deserialize_string_lossy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}
```

Other fields (`vendor_id`, `product_id`, `release_number`, `serial_number`, `port`, `driver`, `experimental`) are intentionally not deserialized — serde_json ignores unknown keys by default. Keep the existing `DeviceEntry` (status parsing) and `StatusEntry` untouched — different commands, different schemas, different structs.

**Subprocess serialization (concurrency safety).** `liquidctl` opens an exclusive `O_RDWR` claim on `/dev/hidrawN` per device via HIDAPI. If the popup-open enumeration fires while the status poll is mid-flight on the same device, the second invocation can fail with a permission/busy error — surfacing as a spurious `last_error`. To eliminate this race, **serialize all liquidctl subprocess calls behind a module-private async mutex**:

```rust
use tokio::sync::Mutex;

static LIQUIDCTL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub async fn fetch_status(match_filter: &str) -> Result<AioStatus, Error> {
    let _guard = LIQUIDCTL_LOCK.lock().await;
    // … existing body …
}

pub async fn list_devices() -> Result<Vec<DetectedDevice>, Error> {
    let _guard = LIQUIDCTL_LOCK.lock().await;
    // … new body …
}
```

The lock is held only for the duration of the subprocess call (≤ 3 s for status, ≤ 1 s for list). Since the existing `Subscription` is single-threaded and only one `Task::perform` enumerate runs at a time, contention is bounded to "popup-open enumerate vs in-flight poll" — at worst the enumerate waits one poll-cycle.

Tests added (mirrors existing parser test style):

- `parses_multiple_devices_from_list_fixture` — fixture is the real `[{"description": "Gigabyte RGB Fusion …"}, {"description": "Corsair Hydro H150i Pro XT", …}]` output captured from this machine
- `empty_list_yields_empty_vec` (distinguishes from `Error::NoDevice` in the `fetch_status` path)
- `malformed_list_yields_parse_error`
- `list_device_entry_ignores_unknown_fields` — confirms vendor_id/driver/experimental are silently dropped

### 3. `src/devices.rs` — classification helper

```rust
const AIO_PATTERNS: &[&str] = &[ ... ];  // see above; written lowercase

pub fn is_aio(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    AIO_PATTERNS.iter().any(|p| d.contains(p))
}

/// Filter the enumerated device list to AIOs only, preserving liquidctl's order.
pub fn filter_aios(devices: &[DetectedDevice]) -> Vec<&DetectedDevice> {
    devices.iter().filter(|d| is_aio(&d.description)).collect()
}

/// Pick the first AIO from a list, or None if no AIO is present.
pub fn auto_select(devices: &[DetectedDevice]) -> Option<&DetectedDevice> {
    devices.iter().find(|d| is_aio(&d.description))
}
```

Tests (pure functions, no subprocess):

- `is_aio_matches_known_substrings` — Hydro / iCUE H / Kraken / D5 Next / etc.
- `is_aio_rejects_psus_and_rgb_hubs` — Corsair RMi, Lighting Node Pro, RGB Fusion controller
- `is_aio_is_case_insensitive` — `"corsair hydro h150i"` matches
- `auto_select_picks_first_aio_in_list` — leading non-AIO entries are skipped
- `auto_select_returns_none_for_no_aio` — list of only PSUs / RGB controllers

### 4. `src/app.rs` — model, messages, UI, subscription

#### New / modified imports

```rust
use crate::liquidctl::DetectedDevice;
use crate::devices;
use cosmic::iced::Task;     // (already present transitively)
```

#### New `AppModel` fields

```rust
/// Devices observed at the last `liquidctl list` enumeration. Filtered
/// to AIOs via `devices::filter_aios` when used by the dropdown.
detected_devices: Vec<DetectedDevice>,
/// True while a `liquidctl list` task is in flight, so the popup can
/// show a "Refreshing…" placeholder and avoid concurrent requests.
device_scan_in_flight: bool,
```

#### New `Message` variants

```rust
/// Result of a `liquidctl list --json` enumeration.
DevicesEnumerated(Result<Vec<DetectedDevice>, String>),
/// User chose a device from the popup dropdown. `None` means revert to Auto.
DeviceSelected(Option<String>),
```

#### Init

`AppModel::init` is changed to return a real startup `Task` that calls `liquidctl::list_devices()` and dispatches `DevicesEnumerated`. This kicks the auto-detect off before the first poll tick lands. **Set `device_scan_in_flight = true` on the model before returning** so the popup (if opened during the brief startup window) shows "Detecting devices…" rather than "No supported AIO detected".

```rust
let app = AppModel {
    core,
    config,
    config_handle,
    device_scan_in_flight: true,
    ..Default::default()
};

let init_task = Task::perform(
    async { crate::liquidctl::list_devices().await.map_err(|e| format!("{e}")) },
    |r| cosmic::Action::App(Message::DevicesEnumerated(r)),
);
(app, init_task)
```

Verified at `iced/runtime/src/task.rs:48`: `Task::perform(future, f)` exists with the expected signature.

#### `update` handlers

The handlers must clear stale per-device histories whenever the **effective device** changes. Today `temp_history` / `pump_duty_history` / `fan_avg_duty_history` and `last_status` are tied implicitly to one device; if we let them carry across a device change, the new device's sparkline will start with bogus left-side data from the old device — visually wrong and misleading. A small private helper keeps the clearing in one place:

```rust
fn reset_device_state(&mut self) {
    self.temp_history.clear();
    self.pump_duty_history.clear();
    self.fan_avg_duty_history.clear();
    self.last_status = None;
    self.last_error = None;
}
```

```rust
Message::DevicesEnumerated(Ok(devs)) => {
    let prev_effective = self.effective_match();
    self.detected_devices = devs;
    self.device_scan_in_flight = false;
    let new_effective = self.effective_match();
    if prev_effective != new_effective {
        // Auto-detect now points at a different device (or now points
        // at one when previously it didn't). Drop stale samples so the
        // sparklines and last_status reflect only the new device.
        self.reset_device_state();
    }
    // If enumerate completed and no AIO is connected at all, surface a
    // clear, one-shot error so the user isn't stuck on `…` forever.
    if new_effective.is_none() {
        self.last_error = Some(
            "no supported AIO detected — open the popup to select a device".to_string()
        );
    }
}
Message::DevicesEnumerated(Err(msg)) => {
    self.last_error = Some(msg);
    self.device_scan_in_flight = false;
}
Message::DeviceSelected(choice) => {
    if self.config.device_match != choice {
        // Compute the effective-match delta BEFORE mutating config so
        // we only reset history when the effective device actually
        // changed (e.g. switching between two valid choices), not on
        // semantic no-ops like Auto→explicit-pick-of-the-auto-device.
        let prev_effective = self.effective_match();
        self.config.device_match = choice;
        let new_effective = self.effective_match();
        if prev_effective != new_effective {
            self.reset_device_state();
        }
        if let Some(handle) = self.config_handle.as_ref() {
            let _ = self.config.write_entry(handle);
        }
    }
}
```

`TogglePopup` arm: when **opening** the popup (and `!self.device_scan_in_flight`), also kick off another `list_devices` task so the dropdown reflects hot-plugged state. Set `self.device_scan_in_flight = true` before launching the task. Use `Task::batch` to combine `get_popup` and the enumerate task.

#### Effective match resolution

A new private method:

```rust
fn effective_match(&self) -> Option<String> {
    self.config.device_match.clone()
        .or_else(|| devices::auto_select(&self.detected_devices)
            .map(|d| d.description.clone()))
}
```

#### Subscription

Re-key on `(interval_ms, String)`. `Subscription::run_with` requires only `D: Hash + 'static` (verified at `iced/futures/src/subscription.rs:198` — no `Send + Sync + Clone` bound). The fn pointer is non-capturing — clone the string out inside:

```rust
// effective_match() returns Option<String>. Until enumeration completes
// (or if no AIO is connected at all), match_str is None and we install
// NO poll subscription — just the config watcher. This avoids the
// startup race where the poll would otherwise fire once with
// "no AIO device detected" before init's enumerate Task lands.
let mut subs: Vec<Subscription<Message>> = vec![
    self.core()
        .watch_config::<Config>(Self::APP_ID)
        .map(|update| Message::UpdateConfig(update.config)),
];

if let Some(match_str) = self.effective_match() {
    let interval_ms = self.config.sample_interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
    let key: (u64, String) = (interval_ms, match_str);

    subs.push(Subscription::run_with(key, |key: &(u64, String)| {
        let interval_ms = key.0;
        let match_str = key.1.clone();
        cosmic::iced::stream::channel(4, move |mut channel: mpsc::Sender<Message>| async move {
            loop {
                let result = crate::liquidctl::fetch_status(&match_str)
                    .await
                    .map_err(|e| format!("{e}"));
                if channel.send(Message::StatusTick(result)).await.is_err() { break; }
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
            futures_util::future::pending().await
        })
    }));
}

Subscription::batch(subs)
```

Two correctness properties this gives us:

1. **No spurious startup error.** Until `init`'s `Task::perform(list_devices, …)` returns and `effective_match()` becomes `Some`, no poll subscription exists, so `last_error` is never set to "no AIO device detected" on first launch. The panel button shows the existing `…` waiting glyph (`view()` already handles `(None, None)` → `…`) until the first successful poll lands.
2. **Re-keying.** When the user commits a new interval OR picks a different device, the `(u64, String)` key changes and iced tears down the old subscription stream and starts a new one. The pattern matches the existing interval re-key idiom documented in `doc/loom/knowledge/patterns.md` ("Subscription Restart on Config Change").

#### Dropdown UI in `popup_metrics_view`

Inserted as a new `column!` section between the existing "Sample interval" slider section and the optional error caption.

**Widget choice — `cosmic::widget::dropdown`.** Verified at `libcosmic/src/widget/dropdown/mod.rs:23`, the signature is:

```rust
pub fn dropdown<S: AsRef<str> + Clone + Send + Sync + 'static, M: Clone + 'static>(
    selections: impl Into<Cow<'a, [S]>>,
    selected: Option<usize>,
    on_selected: impl Fn(usize) -> M + Send + Sync + 'static,
) -> Dropdown<'a, S, M, M>
```

The dropdown emits the **selected index** (a `usize`), not the string; we map index → `Option<String>` ourselves. With ≤ ~6 AIO families v1, the inline menu fits comfortably inside the popup; if v2 broadens the catalog and clipping becomes an issue, switch to `widget::popup_dropdown` (same file, `:37`) which renders the menu in its own Wayland popup. No `Message::Surface` variant is required for the inline `dropdown`.

**Dropdown item construction:**

```rust
fn device_dropdown_items(&self) -> Vec<String> {
    let aios: Vec<&DetectedDevice> = devices::filter_aios(&self.detected_devices);
    let auto_label = match devices::auto_select(&self.detected_devices) {
        Some(d) => format!("Auto ({})", d.description),
        None    => "Auto (no AIO detected)".to_string(),
    };

    let mut items = vec![auto_label];
    items.extend(aios.iter().map(|d| d.description.clone()));

    // If the user has a saved choice that isn't currently connected,
    // surface it as a synthetic "<X> (disconnected)" entry so the UI
    // truthfully reflects that polling is still attempting it. Without
    // this, the dropdown would silently pre-select Auto while the
    // subscription kept polling the saved (missing) device — confusing.
    if let Some(saved) = self.config.device_match.as_ref()
        && !aios.iter().any(|d| &d.description == saved)
    {
        items.push(format!("{saved} (disconnected)"));
    }
    items
}
```

**Selected index resolution:**

- Item 0 = Auto.
- Items 1..N = currently-connected AIOs.
- Item N+1 (only if appended) = the disconnected saved device.

```rust
fn device_dropdown_selected(&self, items: &[String]) -> Option<usize> {
    match self.config.device_match.as_deref() {
        None        => Some(0),  // Auto
        Some(saved) => items.iter()
            .position(|s| s == saved
                       || s == &format!("{saved} (disconnected)")),
    }
}
```

**`on_selected` closure:** map index → `Option<String>`:

```rust
move |idx| {
    let choice = if idx == 0 {
        None
    } else {
        // Strip a trailing " (disconnected)" suffix if present so the
        // saved value round-trips identically.
        items.get(idx).map(|s| s
            .strip_suffix(" (disconnected)")
            .unwrap_or(s)
            .to_string())
    };
    Message::DeviceSelected(choice)
}
```

**Empty-state fallback:** if `self.detected_devices` is empty AND `self.config.device_match.is_none()`, render `widget::text::caption("Detecting devices…")` (during the brief startup window, while `device_scan_in_flight` is true) or `widget::text::caption("No supported AIO detected")` (after the first enumerate completes with no AIOs) instead of the dropdown. Use `device_scan_in_flight` to distinguish.

**Saved-but-disconnected behavior is consistent across `effective_match` and the dropdown:** `effective_match()` always honors `config.device_match` if set, so the poll subscription keeps trying the saved device. The dropdown surfaces this state via the appended `"<saved> (disconnected)"` synthetic entry, pre-selected so the user sees their choice is still in effect. Choosing Auto (index 0) clears the saved value and the synthetic row vanishes on the next render.

### 5. New tests in `src/app.rs`

Following the existing `mod tests` style (model construction via `AppModel::default()`, `cosmic::Application as _` import, no Wayland surface):

- `device_selected_some_persists_choice` — `DeviceSelected(Some("X".into()))` updates `config.device_match`.
- `device_selected_none_clears_choice` — `DeviceSelected(None)` after a prior set reverts to auto.
- `device_selected_same_value_is_noop` — no spurious config write when value unchanged.
- `device_selected_change_resets_history` — switching from one valid device to another clears `temp_history`, `pump_duty_history`, `fan_avg_duty_history`, and `last_status`.
- `device_selected_to_auto_when_auto_resolves_to_same_does_not_reset` — semantic no-op when explicit pick == auto's pick.
- `devices_enumerated_ok_replaces_list` — `DevicesEnumerated(Ok(_))` populates `detected_devices` and clears `device_scan_in_flight`.
- `devices_enumerated_change_in_auto_resets_history` — when auto-detect now resolves to a different device, history clears.
- `devices_enumerated_no_aio_sets_error` — empty enumerate result (and no saved choice) sets `last_error` to the "no supported AIO detected" message.
- `devices_enumerated_err_sets_error_preserves_status` — error variant sets `last_error` without clobbering `last_status`.
- `effective_match_prefers_user_choice_over_auto` — config wins even if auto-detect would pick differently.
- `effective_match_falls_back_to_auto_when_unset` — None config + AIO present → auto's description.
- `effective_match_is_none_when_no_aio_detected` — empty `detected_devices` and no saved choice → None.
- `effective_match_honors_saved_when_disconnected` — saved choice not in `detected_devices` still returned; no silent fallback to auto.
- `update_config_preserves_device_match_when_replaced` — `UpdateConfig` arm round-trips the new field.

Tests for `src/devices.rs::device_dropdown_items` (in `src/app.rs::tests` since it's a method on `AppModel`):

- `dropdown_items_includes_auto_first` — index 0 is always the Auto label.
- `dropdown_items_appends_disconnected_synthetic_when_saved_missing` — saved choice not in detected list is appended with `(disconnected)` suffix.
- `dropdown_items_omits_synthetic_when_saved_is_connected` — no duplicate when saved choice is also in `detected_devices`.

## Verification

End-to-end test plan:

1. **Unit tests:** `cargo test --all-features --no-fail-fast` — all new tests in `src/devices.rs`, `src/liquidctl.rs::tests::{parses_multiple_devices_from_list_fixture, empty_list_yields_empty_vec, malformed_list_yields_parse_error, list_device_entry_ignores_unknown_fields}`, and `src/app.rs::tests::{device_*, dropdown_items_*, effective_match_*}` pass.
2. **Lint gate (CI parity, not `just check`):** the existing `just check` only emits pedantic warnings; CI gates on `cargo clippy --all-targets --all-features -- -D warnings` with `RUSTFLAGS=-D warnings` (per `doc/loom/knowledge/architecture.md` CI section). Run that exact command locally before pushing. Also run `cargo fmt --all -- --check`.
3. **Release build smoke:** `cargo build --release` succeeds.
4. **Manual happy path** (Hydro H150i Pro XT on this machine): `just run`, open popup, confirm dropdown shows `Auto (Corsair Hydro H150i Pro XT)` selected by default, status shows liquid temp / pump / fans as before.
5. **Manual override:** in the dropdown, force-select the same description as a non-Auto entry, confirm config persists across applet restart (`pkill liquidmon && just run`).
6. **Manual reversion:** select Auto, confirm `device_match` is cleared from config (inspect the cosmic-config storage at `~/.config/cosmic/com.github.cosmix.LiquidMon/v3/device_match` is removed or empty).
7. **Manual no-AIO simulation:** create a temporary directory with a stub `liquidctl` shell script that emits `[]` for `list --json` and exits 1 for `status`, then run `PATH=/tmp/stub-liquidctl-dir:$PATH just run`. Confirm the popup shows `"No supported AIO detected"`, the panel shows `!`, no panic. **Do NOT rename `/usr/bin/liquidctl`** — affects every other process on the machine and risks lingering breakage if the test crashes mid-way.
8. **Hot-plug simulation:** with applet running, close popup, observe a second `liquidctl list` runs the next time the popup is opened (trace via `strace -f -e execve -p $(pgrep liquidmon)` or by adding a temporary `eprintln!` in `list_devices`).
9. **Config migration:** drop a v2 config file (no `device_match` key) into the COSMIC config dir; launch applet; confirm it loads with `sample_interval_ms` preserved and `device_match: None`.
10. **Concurrency safety:** run with a low sample interval (1 s); rapidly toggle the popup open/closed. Confirm no `liquidctl` "device busy" or permission errors appear in `last_error`. The `LIQUIDCTL_LOCK` mutex is the protection here — if test fails, the mutex isn't being held.
11. **Subscription re-key (no double-poll):** with poll interval at 5 s, switch device in dropdown; confirm via `eprintln!` or strace that the OLD subscription stops issuing `liquidctl status` calls and the NEW one starts using the new `--match` value, with no overlap.
12. **History reset on device change:** seed `temp_history` with samples on device A, switch dropdown to device B, confirm sparkline starts blank (single-tick rendering after first poll on B), not carrying device A's trail.

## User-Facing Metadata Updates

The applet's marketing/metadata strings still describe a Corsair-Hydro-only tool. Once v1 broadens supported devices to include Corsair iCUE Elite (and v2 will go further), all four user-facing surfaces must be updated to honestly describe the supported set:

| File                              | Current text                                                                                                                                  | New text (v1)                                                                                                                                                                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml:6` (`description`)    | `COSMIC panel applet for Corsair AIO temps and fan/pump duties via liquidctl`                                                                 | `COSMIC panel applet for AIO liquid cooler temps and fan/pump duties via liquidctl`                                                                                                                                                             |
| `Cargo.toml:16` (extended-desc)   | "LiquidMon is a COSMIC panel applet that surfaces Corsair AIO liquid …"                                                                       | "… that surfaces AIO liquid cooler …" (drop "Corsair", add: "Currently supports Corsair Hydro Pro/Pro XT/Platinum and iCUE Elite families; broader liquidctl device support is planned.")                                                       |
| `resources/app.desktop:3`         | `Comment=COSMIC panel applet for Corsair AIO temps and fan/pump duties via liquidctl`                                                         | `Comment=COSMIC panel applet for AIO liquid cooler temps and fan/pump duties via liquidctl`                                                                                                                                                     |
| `resources/app.metainfo.xml:7`    | `<summary>COSMIC panel applet for Corsair AIO temps …</summary>`                                                                              | `<summary>COSMIC panel applet for AIO liquid cooler temps and fan/pump duties via liquidctl</summary>`                                                                                                                                          |
| `README.md:3`                     | `LiquidMon is a COSMIC panel applet that monitors Corsair Hydro AIO coolers in …`                                                             | `LiquidMon is a COSMIC panel applet that monitors AIO liquid coolers in …`                                                                                                                                                                      |
| `README.md:21-24`                 | "Any Corsair AIO whose `liquidctl status` description contains the word `Hydro` … the device match filter is currently hardcoded to `Hydro`." | Replace with: list of currently-supported families (Corsair Hydro Pro / Pro XT / Platinum and iCUE Elite Capellix / RGB), describe Auto behavior, describe device-selector dropdown, and note that broader liquidctl device support is planned. |
| `README.md:102` (example command) | `liquidctl --match Hydro --json status`                                                                                                       | Update to show `liquidctl list --json` for enumeration plus a per-device `--match "<full description>"` example                                                                                                                                 |

These updates land in the same PR as the code so nobody sees a release where the dropdown supports iCUE but the README still says Hydro-only.

## Knowledge Updates (post-completion)

After the feature lands, update `doc/loom/knowledge/`:

- **`architecture.md`** — Update the Module Dependency Graph (add `mod devices`); update "Cross-Cutting Concerns Synthesis → Partially Hardcoded Config" entry to remove the device-filter line; add a "Device Enumeration" subsection under "Data Flow" describing `liquidctl list --json` flow and the `effective_match` resolution.
- **`entry-points.md`** — Add `src/devices.rs` to the reading order; add `DetectedDevice` to the Key Types table; document `Message::DevicesEnumerated` and `Message::DeviceSelected` in the message dispatch path.
- **`patterns.md`** — Append "Device Auto-Detection via Substring Catalog" pattern documenting `AIO_PATTERNS` and the description-as-`--match` decision.
- **`concerns.md`** — Mark "Hardcoded device match string" as RESOLVED (with date 2026-05-01 or current); add any new concerns surfaced (e.g. the "no auto-detect → no poll" fallback choice).
- **`stack.md`** — No change expected (no new dependencies).
- **`mistakes.md`** — Append only if anything went wrong during implementation worth a prevention rule.
- **`conventions.md`** — Append `Option<T>`-as-config-default convention if it's the first such field (it is: `sample_interval_ms` is the only existing field and is non-Option).

## Out of Scope (Deferred to Follow-up Plans)

- **Broader liquidctl device support** — Aquacomputer, NZXT Kraken, EVGA CLC, MSI Coreliquid, ASUS Ryujin, Lian Li Galahad. Requires `Pump.duty_pct: Option<u8>`, `Fan.duty_pct: Option<u8>`, recognition of `"Coolant temperature"` as a `"Liquid temperature"` synonym, and handling of non-indexed `"Fan speed"` keys. Tracked as `PLAN-aio-broad-support.md` (to be authored).
- **Truly-unique device selection** — `liquidctl --match` is substring matching. With two identical AIOs (e.g. dual H150i Pro XT in a workstation), liquidctl picks the first regardless of saved description. Fixing this requires storing `(description, bus, address)` and invoking liquidctl with `--bus X --address Y` instead of `--match`. Tracked as a follow-up — most users have one cooler, and the v1 dropdown still surfaces both devices honestly so the user knows there's an ambiguity.
- **Cooler control** (setting pump/fan curves) — this PR is read-only, same as today.
- **Multi-device monitoring** — the applet still reads one device at a time; users with two AIOs pick one in the dropdown.
- **Free-text custom `--match` field** — rejected at planning time in favour of the curated dropdown.
- **Live device hot-plug detection without opening the popup** — a periodic 30 s scan was rejected to avoid extra subprocess load and avoid serializing on `LIQUIDCTL_LOCK` during status polls. Hot-plug is discovered when the user next opens the popup.

## Known Limitations (Documented for Users)

These will be added to the README "Limitations" section:

1. **One device per cooler family.** When two identical AIOs are connected, liquidctl's substring match selects the first; v1 cannot disambiguate. (Tracked above.)
2. **Limited family support.** v1 supports Corsair Hydro Pro / Pro XT / Platinum and iCUE Elite Capellix / RGB. Other liquidctl-supported families are detected but not yet shown in the dropdown — broadening is planned. (Tracked above.)
3. **Hot-plug detected on popup open.** Plugging a new cooler in does not auto-update the panel; open the popup once to trigger re-enumeration.
