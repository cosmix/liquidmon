# PLAN: Cooling Controls (Presets + Manual) via liquidctl

## Context

LiquidMon is read-only today: it polls `liquidctl --json status` and renders
coolant temperature, pump duty and fan duties. Every control decision is left
to whatever last wrote to the cooler (BIOS, a previous `liquidctl` invocation,
or the device's power-on defaults).

This plan adds a write path so the popup can drive the cooler: four named
**presets** that push a device-side fan curve plus a pump mode, and a **manual**
mode with a fan-duty slider and a pump-mode selector. All writes shell out to
`liquidctl`, reusing the existing subprocess plumbing and serialization lock.

Verified against the installed toolchain: `liquidctl v1.15.0`, device
`Corsair Hydro H150i Pro XT` on `/dev/hidraw1`.

### Decisions taken before drafting

| Question | Decision |
| --- | --- |
| Device families with write support in v1 | `hydro_platinum` only (Hydro Platinum / Pro XT / iCUE Elite RGB). Everything else stays read-only with an explicit caption. |
| What a preset does | Bundles a pump mode with a 7-point **device-side** fan curve, so the setting survives LiquidMon exiting. |
| Manual granularity | One duty slider driving the `fan` channel (all fans), plus a pump-mode dropdown. No per-fan sliders. |
| Persistence | Mode persisted via cosmic-config. The cooler keeps the setting itself; LiquidMon re-applies only when it observes that the cooler has lost it. |
| Write frequency | No write on a timer, no unconditional write at start. Writes happen on a user gesture, or once per applet run when the device's reported state diverges from the desired state. |

## Driver Constraints (verified in liquidctl 1.15.0 sources)

### Per-family control surface

| Driver | Devices | Fan control | Pump control |
| --- | --- | --- | --- |
| `hydro_platinum.py` | Hydro H60i/H100i/H115i/H150i **Pro XT**, H100i/H115i **Platinum**, iCUE H100i/H115i/H150i **Elite RGB** | `set fan speed <duty>`, `set fanN speed <duty>`, 7-point curve | **No duty.** Only `initialize --pump-mode quiet\|balanced\|extreme` (`_PumpMode`, `hydro_platinum.py:73-81`) |
| `asetek_pro.py` | Hydro H100i/H115i/H150i **Pro** (non-XT) | fixed duty + curve | **No duty** — `_fan_indexes('pump')` raises `NotSupportedByDevice` (`asetek_pro.py:255-256`). Modes `quiet\|balanced\|performance` via `initialize` |
| `commander_core.py` | Commander Core / Core XT / ST (the pump head inside iCUE Elite **Capellix**) | fixed duty + curve, `fan1`–`fan6` | **Duty supported** on `pump` when `has_pump` (`commander_core.py:352-356`). Listed `(broken)` in 1.15.0 |

v1 writes only through the `hydro_platinum` row. The capability lookup is a
pure function so the other two rows drop in later without touching the UI.

### The payload-clobber trap (the single most important constraint)

`hydro_platinum` does not do partial writes. Every `set_fixed_speed` and
`set_speed_profile` call ends in `_send_set_cooling()`, which rebuilds the
**entire** cooling payload from liquidctl's runtime key-value store
(`hydro_platinum.py:405-435`):

- a fan channel with no stored mode falls back to `_FanMode.FIXED_DUTY` with
  `default=100` → **unconfigured fans jump to 100% duty**;
- `pump_mode` with no stored value falls back to `_PumpMode.BALANCED`.

That store lives in `$XDG_RUNTIME_DIR/liquidctl` (`keyval.py:22-44`), so it is
**wiped on every reboot**. Consequences that shape the design:

1. Always write the `fan` channel (all fans at once), never `fanN` — a per-fan
   write after a fresh boot slams the untouched fans to 100%.
2. Pump mode is only settable through `initialize --pump-mode`, and
   `initialize` re-sends the same full payload — so it must run **after** the
   fan write, not before. Fan-first ordering costs at most one poll interval of
   `balanced` pump on a cold store; pump-first ordering would cost a burst of
   100% fans.
3. LiquidMon must own the full desired state and send both steps on every
   apply. Cost is ~1 s of held subprocess lock per user gesture.

### Other verified facts

- Curves are `(temperature, duty)` pairs against the cooler's own liquid
  sensor. `normalize_profile` (`util.py`) sorts, forces monotonic non-decreasing
  duty, and appends a `(60, 100)` failsafe; `_prepare_profile` then pads to
  exactly 7 points and raises `ValueError` past that. **Preset curves must have
  at most 6 points** (the failsafe takes the seventh) unless the last point is
  already 100%.
- `--json` covers `list`, `initialize` and `status` only — a `set` reports
  success solely through exit code 0 and an empty stdout.
- `initialize` also stores `leds_enabled = 0`. That is only a "resend the LED
  command next time" flag (`hydro_platinum.py:331-341`); it does not alter the
  LEDs on the device.
- `initialize` warns when firmware < 1.1.0, where "fan settings affect Fan 1
  only and disable Fan 2" (upstream issue #201) — a second reason the UI offers
  one all-fans control rather than per-fan sliders.
- No privilege escalation is needed. `/etc/udev/rules.d/71-liquidctl.rules`
  tags the HID nodes `uaccess`, giving the active session an rw ACL, and
  hidapi already opens the node `O_RDWR` for the status reads that work today.

## Write Persistence and Device Wear

Verified in liquidctl 1.15.0 sources:

- **`hydro_platinum` has no non-volatile write path.** The string
  `non_volatile` appears nowhere in the driver. `set_fixed_speed`,
  `set_speed_profile` and `initialize` all end in one
  `_send_command(_FEATURE_COOLING, _CMD_SET_COOLING, …)` that pushes the
  payload into the controller's working state. There is no EEPROM or flash
  write, and therefore no write-endurance budget to spend on this family.
- The `--non-volatile` CLI flag (`cli.py:41,121`) reaches exactly two drivers:
  `asetek.py` (legacy Asetek 690LC — NZXT Kraken X41/X61, EVGA CLC) and
  `nvidia.py`. liquidctl's own comment on that path reads "as this memory has
  some unknown yet limited endurance" — i.e. upstream treats it as wear-prone.
  **LiquidMon never passes `--non-volatile`**, on any family, ever. If a future
  family makes a persistent save genuinely useful, it ships as a one-shot
  button behind an explicit confirmation, never as part of an automatic path.
- `commander_core` writes "HW" speed modes and curves with no non-volatile
  flag. Whether the Commander Core backs those with flash is **unverified**;
  that uncertainty is one more reason its write path stays out of v1.

What "the cooler keeps the setting" means on this family: the controller holds
the curve and pump mode as long as it has power. That covers an applet restart,
a logout, and an OS reboot on any system where the PSU keeps +5 V standby
alive. It is lost on a full power cut (PSU switch, wall plug, battery pull).

The wear argument does not apply to the Pro XT, but the rewrite-avoidance rule
is kept anyway, because it is correct for three independent reasons: each write
holds the liquidctl lock for ~1 s and delays a status sample; a blind write at
every start would stomp a setting the user deliberately made elsewhere between
sessions; and the policy must already be safe for the families this plan leaves
room for. The rule, stated once:

> LiquidMon writes to the cooler on a user gesture, or **at most once per
> applet run** when it has positive evidence the cooler is not running the
> desired setting. It never writes on a timer, never writes at start
> unconditionally, and never writes when the observed state already matches.

### Divergence detection

The status poll is the readback channel. `Fan N duty` is the firmware's
*commanded* duty, not a measurement, so it tracks the active curve or fixed
duty directly (round-tripped through a `/255` byte, hence ±1%).

- **Manual / `Max`** (fixed duty `d`): diverged when every reported fan duty
  differs from `d` by more than 2 percentage points.
- **Curve presets**: expected duty = linear interpolation of the preset curve
  at the reported liquid temperature. Diverged when the mean reported fan duty
  differs from that by more than 6 points — wide enough to absorb the
  firmware's own interpolation and the lag between a temperature sample and the
  duty it produced.
- Divergence must hold for **two consecutive status samples** before a write is
  dispatched, so a single transient (a sample taken mid-ramp, or right after a
  resume) cannot trigger one.
- **Pump mode is not readable on this family** — status reports pump duty and
  rpm, never the mode, and the duty a mode produces varies by model. Pump mode
  is therefore only ever written as the second step of a write that the fan
  check already justified, or on a direct user gesture. An externally changed
  pump mode is not detected; the popup's "Apply now" button is the answer.

### Per-boot fast path

Before any of that, LiquidMon checks a marker file at
`$XDG_RUNTIME_DIR/liquidmon/applied` holding
`<device description>\n<settings fingerprint>`. Written after a successful
apply, it sits in the same tmpfs liquidctl uses for its own runtime store, so
it disappears on reboot exactly when liquidctl's stored state does. A matching
marker means this boot already applied these settings to this device: skip the
divergence check entirely and write nothing. The marker is an optimisation, not
the authority — a missing marker still only produces a write if divergence is
actually observed.

## Control Model

```rust
/// What LiquidMon is currently driving the cooler with.
pub enum ControlMode {
    /// LiquidMon never writes to the device (default, and what every
    /// existing install upgrades into).
    Unmanaged,
    Preset(Preset),
    Manual,
}

pub enum Preset { Silent, Balanced, Performance, Max }

pub enum PumpMode { Quiet, Balanced, Extreme }
```

### Preset table

Curve points are `(liquid °C, fan duty %)`. Each row is already monotonic and
≤ 6 points, so `_prepare_profile` appends `(60, 100)` without overflowing.

| Preset | Fan curve | Pump mode |
| --- | --- | --- |
| Silent | (25,20) (30,25) (35,35) (40,50) (45,75) | Quiet |
| Balanced | (25,25) (30,35) (35,50) (40,70) (45,90) | Balanced |
| Performance | (25,40) (30,55) (35,70) (40,85) (45,100) | Extreme |
| Max | fixed 100% (no curve) | Extreme |

`Max` uses `set fan speed 100` rather than a one-point curve: the intent is a
flat wall, and the fixed-duty path is the one the driver models directly.

### Manual mode

- Fan duty slider, range **20–100%, step 5**, committed on release (mirrors the
  existing `SampleIntervalDragged` / `SampleIntervalReleased` pair). 20% is a
  floor, not a hardware limit — the driver accepts 0, but most 120/140 mm fans
  stall below ~20% and a stalled fan reads as a failure in the popup.
- Pump-mode dropdown: Quiet / Balanced / Extreme.

Manual values and the last-chosen preset are stored independently, so switching
Preset → Manual → Preset never loses either setting.

### Re-apply toggle

One user-visible switch, `auto_reapply`, default **on**:

- **On** — if LiquidMon observes the cooler is not running the saved setting
  (per "Divergence detection"), it writes it once for that applet run.
- **Off** — LiquidMon never writes on its own. The saved mode is still shown,
  and the "Apply now" button writes it on demand.

Either way the write count is bounded: at most one automatic write per applet
run per device, plus whatever the user explicitly asks for.

## Files Modified / Added

| File | Change |
| --- | --- |
| `src/control.rs` | **New.** Mode/preset/pump enums, preset table, family capability lookup, duty clamping, pure argv builders. All logic that can be unit-tested lives here. |
| `src/liquidctl.rs` | Extract a shared `run_liquidctl(args) -> Result<String, Error>`; add `apply_control(match, steps)`. Existing `fetch_status` / `list_devices` refactored onto the shared runner. |
| `src/config.rs` | `#[version = 4]`; add `control_mode`, `control_preset`, `manual_fan_duty`, `manual_pump_mode`. |
| `src/app.rs` | Six new `Message` variants, control state fields (including the stateful `pump_model`), `update` arms, apply dispatch + coalescing, divergence-driven re-apply. |
| `src/control_view.rs` | **New.** `control_section()` and its helpers + tests. Does **not** go in `view.rs` — see the size note below. |
| `src/view.rs` | One line: `popup_metrics_view` calls `control_view::control_section(..)`. Nothing else changes. |
| `src/curve.rs` | **New.** `canvas::Program` drawing the active preset's curve with a marker at the current coolant temperature. |
| `src/main.rs` | `mod control;`, `mod control_view;`, `mod curve;` |
| `README.md`, `resources/app.metainfo.xml`, `Cargo.toml` description, `CHANGELOG.md` | User-facing copy (see below). |

**On file sizes.** `src/app.rs` is 1225 lines, `src/liquidctl.rs` 751 and
`src/view.rs` 411 — all already past the 400-line ceiling, tests included. This
plan does not fix that (a split of `app.rs` is its own change, and mixing it in
would bury the feature diff), but it does not make it worse than it must:
every piece that can live in a new file does, and each new file lands well
inside the limit — `control.rs` ~300, `control_view.rs` ~180, `curve.rs` ~110.
Net growth of the existing files is roughly 140 lines in `app.rs`, 60 in
`liquidctl.rs`, 1 in `view.rs`. The reason `control_section` is NOT in
`view.rs`: adding ~180 lines there would push a 411-line file to ~590 for no
reason other than habit.

## Detailed Design

### 1. `src/control.rs` (new)

```rust
// SPDX-License-Identifier: MPL-2.0

//! Cooling-control model: presets, manual settings, per-family capability
//! lookup, and the pure `liquidctl` argv builders that apply them.

/// Lowest fan duty the UI will request. The driver accepts 0, but most
/// 120/140 mm fans stall below ~20% and a stalled fan reads as a fault.
pub const MIN_FAN_DUTY: u8 = 20;
pub const MAX_FAN_DUTY: u8 = 100;

/// Control capability of the driver behind a device description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// `hydro_platinum`: all-fan duty + curve, pump mode only.
    FanDutyAndPumpMode,
    /// No verified write path — controls render read-only.
    None,
}

/// Classify a liquidctl device description, case-insensitively. Three
/// ordered substring rules — "platinum", "pro xt", "elite rgb" — which
/// together cover every `hydro_platinum` description in liquidctl 1.15.0
/// and exclude `asetek_pro`'s plain "Hydro H150i Pro".
pub fn capability(description: &str) -> Capability { … }

/// The full desired device state. Held in `AppModel`, rebuilt from `Config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mode: ControlMode,
    pub preset: Preset,
    pub manual_fan_duty: u8,
    pub manual_pump_mode: PumpMode,
    /// Whether LiquidMon may write on its own when it observes the cooler
    /// has lost the setting. Never permits more than one write per run.
    pub auto_reapply: bool,
}

impl Settings {
    /// Resolve to the concrete (fan action, pump mode) pair to send, or
    /// `None` when the mode is `Unmanaged`.
    pub fn resolve(&self) -> Option<(FanAction, PumpMode)> { … }

    /// Stable fingerprint of everything that reaches the device. Written to
    /// the per-boot marker so a restart within the same boot can skip the
    /// divergence check entirely. Excludes `auto_reapply`, which changes no
    /// device state.
    pub fn fingerprint(&self) -> u64 { … }
}

pub enum FanAction { Fixed(u8), Curve(&'static [(u8, u8)]) }

/// What the status line under the controls says. Carries the session
/// write count so the line doubles as a receipt for the write policy.
///
/// The timestamp is an `Instant`, rendered relative ("just now",
/// "4 min ago"), NOT a wall-clock time: `std` cannot format local time and
/// this feature does not justify pulling in a date/time crate. The mockup
/// shows "Written 14:32" — that is the one place the implementation
/// deliberately differs from it. While the popup is open the existing
/// `AnimationTick` already re-renders at 33 ms, so the relative label ages
/// on its own with no extra subscription.
pub enum ApplyStatus {
    Never,
    Ok { at: Instant, writes: u32 },
    Failed { stderr: String, writes: u32 },
}

/// Why an apply is being dispatched. Only `User` bypasses the per-run
/// budget and the marker fast path.
pub enum ApplyTrigger { User, Divergence }

/// Fan duty the active setting should be producing at `liquid_temp_c`:
/// the fixed duty, or the curve interpolated linearly between its points
/// (flat below the first point, flat above the last).
pub fn expected_fan_duty(settings: &Settings, liquid_temp_c: f64) -> Option<f64>;

/// Whether a status sample contradicts the active setting, within the
/// tolerance for its kind (2 points fixed, 6 points curve). `None` when
/// there is nothing to compare — unmanaged, or a sample with no fans.
pub fn diverges(settings: &Settings, status: &AioStatus) -> Option<bool>;

/// Build the ordered argv lists for one apply. Fans first, pump second —
/// see "payload-clobber trap". Returns `None` for `Unmanaged` or an
/// uncontrollable device.
pub fn apply_steps(match_str: &str, settings: &Settings, cap: Capability)
    -> Option<Vec<Vec<String>>>
```

Emitted argv, for `Balanced` on `Corsair Hydro H150i Pro XT`:

```text
["--match", "Corsair Hydro H150i Pro XT", "set", "fan", "speed",
 "25", "25", "30", "35", "35", "50", "40", "70", "45", "90"]
["--match", "Corsair Hydro H150i Pro XT", "initialize", "--pump-mode", "balanced"]
```

The twelve descriptions `capability` must accept, copied from
`hydro_platinum.py:118-141` — the exact fixture list for its test:

```text
Corsair Hydro H60i Pro XT          Corsair iCUE H100i Elite RGB
Corsair Hydro H100i Pro XT         Corsair iCUE H115i Elite RGB
Corsair Hydro H115i Pro XT         Corsair iCUE H150i Elite RGB
Corsair Hydro H150i Pro XT         Corsair iCUE H100i Elite RGB (White)
Corsair Hydro H100i Platinum       Corsair iCUE H150i Elite RGB (White)
Corsair Hydro H100i Platinum SE
Corsair Hydro H115i Platinum
```

Per-boot marker, also in `control.rs` (a few lines of `std::fs`, no async — the
file is tens of bytes and is touched at most once per apply):

```rust
/// Directory holding the per-boot apply marker: `$XDG_RUNTIME_DIR/liquidmon`.
/// `None` when `XDG_RUNTIME_DIR` is unset, which degrades to "no fast path"
/// rather than falling back to a persistent location — the marker must die
/// with the boot, or it would suppress a needed write after a power cut.
pub fn marker_dir() -> Option<PathBuf>;

/// True when this boot already applied `settings` to `description`.
/// Takes the directory rather than reading the environment so the tests
/// can point it at a temp dir — `std::env::set_var` is `unsafe` in edition
/// 2024 and is not worth it for this.
pub fn marker_matches(dir: &Path, description: &str, settings: &Settings) -> bool;

/// Record a successful apply. Best-effort: a failure to write the marker
/// costs one redundant divergence check next run, nothing more.
pub fn write_marker(dir: &Path, description: &str, settings: &Settings);
```

`fingerprint()` hashes the resolved `(FanAction, PumpMode)` — the argv that
would be sent — through `std::hash::DefaultHasher`. That hash is explicitly
not stable across Rust releases, which is fine and in fact wanted: the marker
lives in tmpfs for one boot, and a toolchain change between two runs in the
same boot at worst costs one redundant divergence check.

Injection is structurally impossible: every element is a separate argv entry
handed to `Command`, no shell is involved, and temperatures/duties are `u8`
formatted at the boundary. This closes the residual half of
`concerns.md:45` ("Subprocess command injection risk") for the write path.

### 2. `src/liquidctl.rs`

Extract the shared runner — the lock comment, timeout, kill-on-drop, exit-code
and UTF-8 handling are identical to what `fetch_status` already does:

```rust
/// Run `liquidctl` with `args` under the global serialization lock and
/// return stdout. `timeout` is per-subprocess and clocks only after the
/// lock is acquired (same contract as `fetch_status`).
async fn run_liquidctl(args: &[String], timeout: Duration) -> Result<String, Error>;

/// Apply an ordered list of control invocations, stopping at the first
/// failure. Holds the lock across the whole sequence so a status poll
/// cannot interleave between the fan write and the pump write.
pub async fn apply_control(steps: Vec<Vec<String>>) -> Result<(), Error>;
```

- Status/list keep their 3 s timeout. Control calls get **5 s**: `initialize`
  does a full cooling write plus a firmware read and is measurably slower than
  a status poll.
- Holding the lock across both steps is deliberate — a status poll landing
  between the fan write and the pump write would report the transient
  `balanced` pump mode on a cold runtime store.
- `Error` gains no variants; `NonZeroExit { status, stderr }` already carries
  what a failed `set` produces on stderr, trimmed by `last_lines`.

### 3. `src/config.rs`

```rust
#[version = 4]
pub struct Config {
    pub sample_interval_ms: u64,
    pub device_match: Option<String>,
    /// Active control mode. `Unmanaged` means LiquidMon never writes.
    pub control_mode: ControlMode,
    /// Last preset chosen, retained across a Manual detour.
    pub control_preset: Preset,
    /// Last manual fan duty, clamped to [MIN_FAN_DUTY, MAX_FAN_DUTY].
    pub manual_fan_duty: u8,
    pub manual_pump_mode: PumpMode,
    /// Allow one automatic write per applet run when the cooler is observed
    /// to have lost the setting. `false` means writes are user-initiated only.
    pub auto_reapply: bool,
}
```

Defaults: `Unmanaged`, `Balanced`, `50`, `PumpMode::Balanced`, `true`.
`Unmanaged` by default matters — an upgrade must never start writing to
hardware on its own, and with `Unmanaged` the `auto_reapply` default is inert
until the user picks a mode.

The three new enums derive `Serialize, Deserialize, Debug, Clone, Copy,
PartialEq, Eq` (cosmic-config stores each struct field as its own RON file, so
each field type must round-trip through serde independently). They are unit
enums, so each persists as a bare RON identifier — `Balanced`, `Manual`.

**Migration from v3.** No migration code is needed and none should be written.
`CosmicConfigEntry::get_entry` reads each field independently and falls back to
that field's `Default` value when its file is absent, which is exactly the
existing behaviour the test `config_default_uses_migration_fallback_values`
pins. An existing v3 config therefore opens as: the user's saved interval and
device, `Unmanaged`, and the defaults above — no write to the cooler, nothing
lost. Extend that test with the new fields rather than adding a second one.

### 4. `src/app.rs`

New `Message` variants:

```rust
/// User picked an entry in the cooling-mode dropdown.
ControlModeSelected(control::ControlMode),
/// Fired continuously while the manual fan slider is dragged.
ManualFanDragged(f32),
/// Fired once on release — clamps, persists, and dispatches an apply.
ManualFanReleased,
/// User picked a pump mode in manual mode.
ManualPumpSelected(control::PumpMode),
/// User pressed "Apply now" — writes unconditionally, bypassing both the
/// per-boot marker and the divergence check.
ApplyRequested,
/// User toggled automatic re-apply.
AutoReapplyToggled(bool),
/// Result of an apply, paired with the match string that started it so a
/// late result from a previous device is ignored (same guard as StatusTick).
ControlApplied { match_str: String, result: Result<(), String> },
```

New `AppModel` fields:

```rust
/// Manual-slider value while mid-drag; `None` outside a drag. Mirrors
/// `pending_interval_secs` so the value only commits on release.
pending_fan_duty: Option<f32>,
/// True while an apply subprocess sequence is in flight.
apply_in_flight: bool,
/// Latest settings requested while an apply was in flight. Coalesces a
/// burst of gestures into one follow-up write instead of a queue.
apply_queued: bool,
/// Device the automatic write budget for this run has been spent on.
/// Set when an automatic apply is *dispatched*, so one failing device
/// cannot spin. Cleared only when the effective device changes.
auto_applied_for: Option<String>,
/// Consecutive status samples that contradicted the active setting.
/// An automatic write needs two; any matching sample resets it to 0.
divergent_samples: u8,
/// Outcome of the last apply plus the session write count, rendered as the
/// status line under the controls.
last_apply: control::ApplyStatus,
/// Stateful model behind the manual pump segmented control. Built in
/// `init`; `activate()` must be called on every path that changes the
/// pump mode, or the segments render without moving the selection.
pump_model: segmented_button::SingleSelectModel,
```

Dispatch helper:

```rust
/// Build and dispatch an apply for the current settings. No-ops when the
/// mode is `Unmanaged`, no device is selected, or the device's family has
/// no verified write path. Coalesces when one is already in flight.
/// `trigger` records whether this came from the user or from divergence;
/// only user-initiated applies bypass the per-run budget.
fn dispatch_apply(&mut self, trigger: ApplyTrigger) -> Task<cosmic::Action<Message>>;
```

Write policy — the whole of it, in the order the checks run:

1. **User gestures always write.** `ControlModeSelected`, `ManualFanReleased`,
   `ManualPumpSelected` and `ApplyRequested` dispatch immediately, ignoring the
   marker, the divergence counter and the per-run budget. The user asked.
2. **Nothing is written at start or on device change.** `DevicesEnumerated` and
   `DeviceSelected` only reset `auto_applied_for` and `divergent_samples` when
   the effective match changes. No apply is dispatched from either arm.
3. **`StatusTick` is the only automatic trigger.** On each successful sample,
   when the mode is not `Unmanaged`, `auto_reapply` is on, the device is
   controllable, and `auto_applied_for` is not already this device:
   - **first sample for this device only**, read the per-boot marker. If it
     matches `(description, fingerprint)`, set `auto_applied_for` to this
     device and stop — this boot already applied these settings, the budget is
     spent, nothing more happens. The marker is read **once per device per
     run**, never on every tick: that check is file IO and the poll runs every
     1.5 s.
   - otherwise feed the sample to `control::diverges`. A matching sample resets
     `divergent_samples` to 0; a diverging one increments it. At 2, dispatch
     one automatic apply and set `auto_applied_for`.
4. **At most one automatic write per device per applet run.** Set on dispatch,
   not on success, so a device that rejects every write is asked exactly once.
   The failure stays in the popup; "Apply now" is the retry.
5. **No timer ever dispatches a write.** There is no retry loop, no periodic
   reconcile, no write from the animation tick or the config watcher.
6. `ControlApplied` with a stale `match_str` is dropped, exactly like
   `StatusTick`. On a live result it clears `apply_in_flight`, updates
   `last_apply`, and — on success — writes the per-boot marker and resets
   `divergent_samples` to 0. A failure leaves the marker alone.
7. **`apply_queued` is drained in the `ControlApplied` arm.** A gesture that
   arrives mid-write sets the flag instead of stacking a task; when the write
   returns, the arm clears the flag and dispatches exactly one more apply with
   the settings as they stand at that moment. Nothing else reads the flag,
   and it never queues more than one.
8. **`UpdateConfig` never dispatches a write.** An external config change (a
   second instance, a settings edit) updates `self.config`, rebuilds
   `Settings`, and calls `pump_model.activate(..)` so the segments track it —
   but a config change is not a user gesture, and the divergence path will
   pick up any real mismatch on its own.
9. No write is ever issued in `Unmanaged`, including at start.

Worst case for a user who leaves a preset on and reboots daily: one write per
boot, and none at all if the cooler kept the setting through standby power.

The control section reads back from the status poll: one interval after a
successful apply the FANS/PUMP metric blocks show the new duties, so no
optimistic overlay is needed — only an "Applying…" caption while in flight.

### 5. `src/control_view.rs` and `src/curve.rs` — the UI

Visual reference, reviewed and approved:
<https://claude.ai/artifact/FW8wB3rEnJgSKmenyhr283> — four artboards (popup
with a preset active and clickable, popup in `Unmanaged`, the four preset
curves, and the edge states).

#### What does not change

The mockup re-draws the existing popup only to place the new section in
context. Nothing above the second divider is in scope:

- **The animated spinner glyphs stay exactly as they are.** `src/spinner.rs`
  and `view::spinner_glyph` — rpm-driven rotation on the PUMP and FANS blocks,
  clocked by `anim_t` — are not touched, not restyled, and not replaced. The
  flat SVG fan and droplet in the mockup are an artifact of drawing it in HTML,
  not a proposal.
- The equalizer blocks, the coolant sparkline, the per-fan rows, the sample
  interval slider and the device dropdown keep their current construction.
- All colour comes from the COSMIC theme, never from the mockup's hex values.
  Those hexes exist only so the mockup renders outside COSMIC; the accent maps
  to `theme.cosmic().accent_color()` (the pattern `src/sparkline.rs:106-112`
  already uses), captions to `widget::text::caption`, body to
  `widget::text::body`, and every readout keeps `.font(cosmic::font::mono())`.

Popup section order becomes: heading → divider → COOLANT / PUMP / FANS blocks →
per-fan rows → divider → **COOLING** → SAMPLE INTERVAL → DEVICE → error caption.
Controls sit directly under the divider because they are now the popup's
primary interaction; the sample-rate and device pickers are configuration.

#### `control_section` signature

```rust
/// Cooling control: a mode dropdown (Unmanaged / four presets / Manual),
/// the active preset's curve preview, the manual fan slider and pump
/// segments, the write-policy row, and the apply-status line. Renders a
/// read-only caption when the device has no verified write path.
pub(crate) fn control_section<'a>(
    settings: &control::Settings,
    capability: control::Capability,
    pending_fan_duty: Option<f32>,
    liquid_temp_c: f64,
    apply_in_flight: bool,
    last_apply: &'a ApplyStatus,
    pump_model: &'a segmented_button::SingleSelectModel,
) -> Element<'a, Message>;
```

`ApplyStatus` is the `control.rs` enum defined in §1 — `Never` / `Ok` /
`Failed`, each carrying the session write count — so the status line is
rendered from one value rather than reconstructed from two `Option`s. Its
three renderings:

```text
(Never)   —  section shows no status line at all
(Ok)      →  "Written just now · 1 write this session"
             "Written 4 min ago · 3 writes this session"
(Failed)  →  "error: liquidctl: no device matches the given filters"
```

#### Layout

```text
COOLING                                          Balanced   ← caption + accent mono
[ Balanced                                            v ]   ← 6-entry dropdown
┌──────────────────────────────────────────────────────┐
│  curve preview, 348 × 46, accent stroke              │   ← presets only
│  20 °C          32 °C · 41%                    60 °C │
└──────────────────────────────────────────────────────┘
Six points written into the cooler. It keeps following them
with LiquidMon closed. Pump: balanced.                       ← caption

FAN DUTY                                             55 %    ← Manual only
[==============o-------------------------------------]
Floor is 20% — most 120 mm fans stall below that.
PUMP
[  Quiet  |  Balanced  |  Extreme  ]                         ← segmented
This family has no pump duty — three modes is all the
firmware exposes.

[x] Re-apply if the cooler loses it          [ Apply now ]
Written 14:32 · 1 write this session                         ← mono caption
```

Per-mode body, one of four, all sharing the header, the write-policy row and
the status line:

| Mode | Body under the dropdown |
| --- | --- |
| `Unmanaged` | One caption: "Not writing to this cooler. The readouts above are whatever it is already running — set by BIOS, another tool, or its own defaults." No write-policy row, no status line. |
| Silent / Balanced / Performance | Curve preview + caption naming the pump mode. |
| `Max` | Caption only: "Fans pinned at 100%, pump on extreme. No curve — the cooler holds this until you change it." |
| `Manual` | Fan duty slider with its floor caption, then the pump segments with theirs. |

#### Widget mapping (verified against the pinned libcosmic rev)

| Element | Call |
| --- | --- |
| Mode picker | `cosmic::widget::dropdown(labels, selected, Message::ControlModeSelected)` — the same widget as the device dropdown. |
| Pump segments | `cosmic::widget::segmented_control::horizontal(pump_model).on_activate(...)` over a `segmented_button::SingleSelectModel`. |
| Re-apply toggle | `cosmic::widget::checkbox("Re-apply if the cooler loses it", settings.auto_reapply, Message::AutoReapplyToggled)`. |
| Apply now | `cosmic::widget::button::standard("Apply now").on_press_maybe(...)` — `None` while in flight or `Unmanaged`, which is how it renders disabled. |
| Fan duty slider | `widget::slider(20.0..=100.0_f32, duty, Message::ManualFanDragged).step(5.0).on_release(Message::ManualFanReleased)` — same shape as `interval_control`. |
| Failed write | `widget::text::caption` in `theme.cosmic().destructive_color()`, holding the `last_lines`-trimmed stderr. |

Two constraints found while checking the widgets, both already reflected above:

1. **`widget::dropdown` renders flat labels only** (`selections: impl Into<Cow<'a, [S]>>` where `S: AsRef<str>`). The mockup's two-line menu entries with a
   grey sub-line are not buildable with it, and a custom popup menu is not
   worth the code here. Ship single-line labels — `Unmanaged`, `Silent`,
   `Balanced`, `Performance`, `Max`, `Manual` — and let the caption *under* the
   dropdown describe the selected mode. That caption is already specified above
   and carries the same information for the one mode that matters.
2. **`SingleSelectModel` is stateful.** It cannot be rebuilt per render like the
   device dropdown's label vector: `pump_model` is an `AppModel` field, built in
   `init` with three entities carrying `PumpMode` data, and `model.activate(..)`
   is called whenever the config load or a `ManualPumpSelected` changes the
   value. Forgetting the activate call is the obvious bug here — the segments
   render, but the selection never moves.

#### `src/curve.rs` (new) — the curve preview

A `canvas::Program<Message, Theme>` in the mould of `src/sparkline.rs`, ~110
lines, drawn at 348 × 46 inside the control section:

- Input: the active preset's points (`&'static [(u8, u8)]`) and the current
  liquid temperature.
- X maps 20–60 °C to the full width, Y maps 0–100% duty to the full height,
  both fixed — the same argument as `equalizer.rs`'s absolute range: an
  auto-scaled axis would make two presets look identical.
- The polyline holds flat from the left edge to the first point (the firmware
  does not ramp below it) and includes liquidctl's appended `(60, 100)`
  failsafe, so what is drawn is what the device received.
- Stroke in `theme.cosmic().accent_color()`, 2 px, round joins; two hairline
  guides at 50% and 75% duty in the theme's divider colour.
- A dashed vertical marker at the current temperature with a filled dot where
  it meets the curve, plus three mono labels below: `20 °C`, the marker's
  `<t> °C · <duty>%`, and `60 °C`.
- Duty at the marker comes from `control::expected_fan_duty` — the same
  function the divergence check uses, so the preview cannot drift from the
  thing that decides whether to write.

A device-side curve is otherwise invisible: without this the user picks
"Balanced" and has no way to know what the cooler will do at 40 °C.

#### Accepted departures from the first draft

Both came out of the mockup and are the version to build:

1. **Pump mode in Manual is a 3-way segmented control**, not a dropdown. Three
   fixed options fit inline, save a click, and read as a mode switch rather
   than a list.
2. **Presets render their curve.** The original plan had presets show only a
   text caption.

## Safety Rules

1. Default is `Unmanaged`; no write happens until the user picks a mode.
2. Fan duty floor of 20% in both the slider and every preset curve.
3. Pump is never set below `Quiet` — the hardware exposes no "off", and no code
   path can produce one.
4. Fans are written as the `fan` channel only; `fanN` is never emitted, so a
   cold runtime store cannot leave an untouched fan at 100%.
5. Every curve is validated monotonic and ≤ 6 points by a unit test, so
   `_prepare_profile` can never reject a preset at runtime.
6. A failed apply surfaces the stderr tail in the popup and stops; it never
   retries on a timer.
7. `--non-volatile` is never passed, on any family. No code path constructs it.
8. At most one automatic write per device per applet run, and none at all
   without two consecutive diverging status samples. User gestures are
   unbounded by design but are one write each.

## Testing

Pure logic in `src/control.rs` carries the weight, matching the existing
convention of testing parsers and helpers rather than subprocesses.

`src/control.rs` (new, ~20 tests):

- every preset curve is monotonic non-decreasing, ≤ 6 points, duty within
  `[MIN_FAN_DUTY, 100]`, temperatures strictly increasing;
- `capability()` returns `FanDutyAndPumpMode` for all twelve `hydro_platinum`
  descriptions in liquidctl 1.15.0, and `None` for `Corsair Hydro H150i Pro`
  (asetek_pro), `Corsair Commander Core (broken)`, and a non-AIO description;
- `apply_steps` argv is exact for each preset, for manual, and is `None` for
  `Unmanaged` and for `Capability::None`;
- fan-first / pump-second ordering is asserted explicitly, with the reason in
  the test name (`apply_steps_writes_fans_before_pump_mode`);
- manual duty clamps at both ends;
- no `apply_steps` output ever contains `--non-volatile`, asserted across every
  mode and preset;
- `expected_fan_duty` interpolates between curve points, holds flat outside
  them, and returns the fixed duty for Manual/`Max`;
- `diverges` is false for a sample matching a fixed duty within 2 points and
  for a sample matching a curve within 6, true past each, and `None` for
  `Unmanaged` and for a fan-less sample;
- `fingerprint` changes with fan action and pump mode, and does **not** change
  with `auto_reapply`;
- the marker round-trips through a `std::env::temp_dir()` subdirectory the test
  creates and removes: written then matched is true, a different device or a
  different fingerprint is false, and a missing directory is false rather than
  a panic.

`src/app.rs` (~10 new tests, reusing the existing `matched_model()` fixture):

- selecting a preset persists it and marks an apply in flight;
- `Unmanaged` dispatches nothing;
- manual drag stages `pending_fan_duty` without persisting; release clamps,
  persists, and dispatches;
- a `ControlApplied` from a stale `match_str` is ignored;
- an error result populates `last_apply` with `Failed` and clears
  `apply_in_flight`; a success increments the write count in `ApplyStatus`;
- `ManualPumpSelected` calls `pump_model.activate(..)` as well as dispatching,
  so the segments and the config never disagree;
- a second gesture during an in-flight apply sets `apply_queued` rather than
  stacking a second task;
- **write-policy tests, the core of this feature's safety:**
  - `DevicesEnumerated` and `DeviceSelected` dispatch **no** apply, even with a
    preset saved and a controllable device;
  - one diverging `StatusTick` dispatches nothing; two consecutive ones
    dispatch exactly one; a matching sample between them resets the counter;
  - after an automatic apply, further diverging samples dispatch nothing for
    the same device (`auto_applied_for` budget), and a device change clears it;
  - with `auto_reapply` off, no number of diverging samples dispatches
    anything, while `ApplyRequested` still does;
  - a matching status sample never dispatches anything, in any mode.

`src/control_view.rs` (~6 tests): dropdown entry order and labels, which body the
section picks per mode (unmanaged caption / curve / `Max` caption / manual
controls), the read-only caption for `Capability::None`, and the status-line
text for each `ApplyStatus` including the `1 write` / `n writes` plural.

`src/curve.rs` (~5 tests, in the style of the existing canvas tests): the
polyline holds flat left of the first point, the `(60, 100)` failsafe is
included, the marker x/y agree with `control::expected_fan_duty` at several
temperatures, and a temperature outside 20–60 °C clamps to the edge rather
than drawing outside the frame.

Subprocess-touching code (`apply_control`) stays untested in CI, as
`fetch_status` and `list_devices` already are; it is covered by the manual
matrix below.

## Verification

```sh
just ci-local     # fmt --check + clippy -D warnings + test + release build
just check        # clippy with -W clippy::pedantic
```

Steps 5–10 below need to see what the applet actually invokes. Put this on
`PATH` ahead of `/usr/bin/liquidctl` and tail the log — the whole harness:

```sh
mkdir -p /tmp/lm-shim && cat > /tmp/lm-shim/liquidctl <<'EOF'
#!/bin/sh
printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >> /tmp/lm-shim/argv.log
exec /usr/bin/liquidctl "$@"
EOF
chmod +x /tmp/lm-shim/liquidctl
# launch the applet with PATH=/tmp/lm-shim:$PATH, then:
rg -v ' (status|list)$' /tmp/lm-shim/argv.log     # every write, nothing else
```

A failing-write run is the same shim with `exec` replaced by
`exit 1` for `set`/`initialize` argv.

Manual matrix on the H150i Pro XT (each step read back with
`liquidctl --match Hydro --json status` and in the popup's own metric blocks):

1. Unmanaged on a fresh install → confirm **no** liquidctl write occurs
   (`ls $XDG_RUNTIME_DIR/liquidctl` unchanged, fan duties untouched).
2. Each preset → fan duty tracks the curve as coolant temperature moves; pump
   rpm shifts between the three modes.
3. Manual at 20 / 50 / 100% → all three fans report the requested duty within
   one poll interval.
4. `rm -rf $XDG_RUNTIME_DIR/liquidctl` (simulating a reboot), then apply →
   confirm no fan spikes to 100% and the pump lands on the requested mode.
5. Restart the applet with a preset saved and the cooler still running it →
   **zero** writes. Verify by pointing `PATH` at a `liquidctl` wrapper that
   logs its argv: only `status` and `list` lines appear.
6. Same wrapper, then force divergence (`liquidctl set fan speed 100` by hand)
   → exactly one `set fan` + one `initialize` pair appears in the log, after
   two poll intervals, and nothing further no matter how long it runs.
7. With `auto_reapply` off, repeat step 6 → no write at all; pressing "Apply
   now" produces exactly one pair.
8. Unplug/replug the cooler's USB header → still no write unless the cooler
   comes back diverged.
9. Reboot with a preset saved → at most one write, and none if the cooler kept
   the setting through standby power.
10. Kill `liquidctl` mid-apply (or point `PATH` at a failing stub) → the popup
    shows the stderr tail, the applet keeps polling, and no retry storm follows
    (the argv log shows one failed attempt, not a stream).

## User-Facing Metadata Updates

| File | Change |
| --- | --- |
| `README.md` | New "Cooling Control" section: the four presets with their curves, manual mode, the 20% floor, that the setting lives in the cooler and keeps running without LiquidMon, that pump control is mode-only on this family, that control needs the same udev/`uaccess` setup as monitoring, and — explicitly — the write policy: no writes on a timer, at most one automatic write per run and only on observed divergence, `--non-volatile` never used. |
| `resources/app.metainfo.xml` | Summary/description currently say monitoring only; add control. New `<release>` entry. |
| `Cargo.toml` | `description` and `extended-description` mention "duty controls" already but the applet did not have them — align the wording with what now ships. |
| `CHANGELOG.md` | **Does not exist in the repo yet** — create it with a single entry for the next version covering this feature. Do not backfill history for earlier releases as part of this change. |

## Documentation Defect Found During Research

`README.md`, `Cargo.toml`, `src/devices.rs` and
`doc/loom/knowledge/architecture.md` all claim support for **"Corsair iCUE
Elite Capellix"**. In liquidctl 1.15.0 the string `Capellix` does not appear
anywhere in the package: the Capellix coolers' pump head is a Commander Core
(USB `1b1c:0c1c`), enumerated by `commander_core.py` as
`Corsair Commander Core (broken)` — which the `"icue h"` pattern in
`AIO_PATTERNS` does not match, and whose status schema is untested against
`parse_status_response`.

What the `"icue h"` pattern actually covers is the **iCUE Elite RGB** family
(`Corsair iCUE H100i/H115i/H150i Elite RGB`, including the White variants),
which `hydro_platinum.py` does support. The fix is a copy change — replace
"Elite Capellix" with "Elite RGB" in the four places above — and is tracked as
a separate, self-contained commit rather than being folded into this feature.

## Implementation Sequence

1. `src/control.rs` + its tests, `mod control;` in `main.rs` — enums, preset
   table, capability lookup, argv builders, `expected_fan_duty` / `diverges`,
   and the per-boot marker. No behaviour change; ships green on its own.
2. `src/liquidctl.rs` runner extraction + `apply_control`. Pure refactor plus
   an unused-until-step-4 function.
3. `src/config.rs` version 4 + defaults, with the existing migration-fallback
   test extended.
4. `src/curve.rs` + its tests, `mod curve;`. Self-contained canvas program,
   green on its own like step 1.
5. `src/app.rs` state/messages/dispatch and `src/control_view.rs` — the step
   that makes the feature visible, and the only one that cannot ship alone.
   Build it against the artboards linked in §5; the animated spinner glyphs
   are not touched.
6. README / metainfo / CHANGELOG copy.

Steps 1–4 each compile and test green on their own, so they can be separate
commits; step 5 is one commit that wires them together. `just ci-local` must
pass at the end of every step, not only at the end.

## Out of Scope (deferred)

- **Resume-aware re-apply.** liquidctl's own docs say the device should be
  re-initialized after suspend-to-RAM; doing it properly needs a logind
  `PrepareForSleep` D-Bus subscription. Deferred. Note that the divergence rule
  already covers the case without it, just two poll intervals later — a resume
  hook would only reset the per-run budget, never bypass the divergence check.
- **Per-fan control.** Blocked on the clobber trap and on the pre-1.1.0
  firmware bug; needs LiquidMon to hold per-channel state it does not have yet.
- **`asetek_pro` and `commander_core` write paths.** The capability enum has
  room for them; neither family is testable on this hardware.
- **Custom user-editable curves.** A curve editor in a panel popup is a
  separate design problem; the four presets cover the common cases first.
- **RGB/lighting control.** Out of the applet's scope entirely.
- **Pump duty slider.** Physically unavailable on `hydro_platinum`; it becomes
  meaningful only when `commander_core` lands.

## Known Limitations (to document for users)

1. Pump control on Hydro Platinum / Pro XT / iCUE Elite RGB is three modes, not
   a percentage — the driver exposes no pump duty write.
2. Applying a setting holds the liquidctl lock for roughly a second, so one
   status sample may arrive late after each change.
3. Settings written into the cooler persist across a LiquidMon restart, a
   logout and (on standby-powered systems) a reboot. They are lost on a full
   power cut, after which LiquidMon re-applies once — two poll intervals after
   it notices, not instantly.
4. If something else changes the **fan** duty (a manual `liquidctl` call, iCUE
   in a VM), LiquidMon treats it as divergence and reclaims the setting once
   per run — not repeatedly, and never if `auto_reapply` is off.
5. If something else changes only the **pump mode**, LiquidMon cannot see it:
   the status output on this family reports pump duty and rpm but never the
   mode. "Apply now" is the remedy.
6. The 6-point tolerance for curve presets means a cooler running a *similar*
   curve set by other software may read as converged and be left alone. That
   is the intended bias: a false "already fine" costs nothing, a false
   divergence costs a write.
