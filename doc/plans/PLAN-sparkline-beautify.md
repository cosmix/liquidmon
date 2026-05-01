# PLAN: Sparkline beautification, popup metrics, sampling-rate config

## Context

Today the panel sparkline is a 1.5 px stroked polyline of liquid temp only. The popup is a flat `list_column` of text rows with no time-series context. The poll interval is hardcoded at 1500 ms and `Config` is empty.

This change:

1. Adds a vertical gradient fill (theme-accent, opaque top → transparent bottom) under the sparkline polyline.
2. Tracks a longer history per metric (temp + pump speed/duty + average fan speed/duty) and renders each as a larger sparkline in the popup.
3. Makes the sample interval user-configurable (1.0 s – 10.0 s, 0.5 s steps) via a COSMIC slider at the bottom of the popup, persisted through cosmic-config.

End state: a more polished panel glyph; a popup that visualises ~15 minutes of every important metric; a settable sample rate that survives restarts.

---

## Files to modify

| File             | Change                                                                                                                       |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `src/sparkline.rs` | Add gradient-fill area under the polyline; expose constructor options (size, accent color override).                       |
| `src/config.rs`    | Add `sample_interval_ms: u64`; bump `#[version]` to `2`; provide a manual `Default` returning `1500`.                      |
| `src/app.rs`       | Add per-metric history buffers, `cosmic_config::Config` handle, popup layout with large sparklines + slider, subscription keyed on the configured interval. |
| `Cargo.toml`       | No new deps. (`iced::widget::canvas::gradient`, `iced::widget::slider`, `cosmic_theme` already pulled in via libcosmic.)   |

No new modules are needed. Stay flat.

---

## Detailed approach

### 1. `src/sparkline.rs` — gradient fill

Currently: builds a single `Path` polyline and calls `frame.stroke(&path, stroke)` with `Color::from_rgba8(180, 200, 230, 0.85)` (`src/sparkline.rs:84-116`).

Confirmed by reading the file: `Sparkline` already implements `canvas::Program<Message, cosmic::Theme>` (`src/sparkline.rs:60`); the existing `_theme: &Theme` parameter is currently unused (`src/sparkline.rs:67`). Drop the underscore and read `theme.cosmic().accent.base` (a `palette::Srgba`) — convert to `iced::Color` via `iced::Color::from(accent)`.

Change:

- Keep the existing polyline `Path` for stroking.
- Build a second `Path` ("area") that retraces the polyline points, then drops to the baseline and closes. Exact path order for the **multi-sample branch**:

  ```text
  move_to(pad,            pad + usable_h)        // baseline, leftmost
  line_to(x0,             y0)                    // up to first sample
  line_to(x1,             y1)
  …
  line_to(x_{n-1},        y_{n-1})               // last sample
  line_to(pad + usable_w, pad + usable_h)        // back down to baseline at right edge
  close()                                         // ties last-baseline back to first-baseline
  ```

  This is a single closed convex-ish polygon along the polyline with the canvas baseline as its lower edge. The closure direction (left-to-right top, right-to-left bottom) is what fills the area "under" the line.

- Build a vertical linear gradient using `cosmic::iced::widget::canvas::gradient::Linear::new(<angle>)`:
  - opaque accent at offset `0.0` (top) — alpha `~0.55`
  - transparent accent at offset `1.0` (bottom) — alpha `0.0`
  - Iced's gradient angle is in radians; vertical "top → bottom" is `std::f32::consts::PI` (verify visually during implementation — flip stops if reversed).

- `frame.fill(&area_path, gradient)` THEN `frame.stroke(&polyline_path, stroke)` so the stroke sits on top of the fill.

- **Single-sample branch** (`src/sparkline.rs:90-100`): replace the bare horizontal tick with a closed rectangle filled by the gradient, plus the tick stroked on top:

  ```text
  area path: move_to(pad,            tick_y)
             line_to(pad + usable_w, tick_y)
             line_to(pad + usable_w, pad + usable_h)
             line_to(pad,            pad + usable_h)
             close()
  stroke path (unchanged): tick line at y across full width
  ```

- Stroke color: use the same accent (alpha ~0.95) instead of the hardcoded light blue.

Add an optional `Sparkline::with_stroke_alpha(f32)` builder so the panel-sized canvas can use a slightly different alpha than the popup-sized canvas if needed (the panel is tiny, the popup is large; same color but tunable). Keep the `samples` constructor signature unchanged.

Existing `y_range(&[f64])` helper (`src/sparkline.rs:35`), the `MIN_Y_SPAN = 2.0` floor, and the single-sample branch are all reused as-is. The gradient pattern must work with all three branches (empty / single / many), confirmed against the rendering grid in `mistakes.md`.

Add unit tests where the math is independent of the renderer:
- a builder test for `with_stroke_alpha`.
- (No render-side test — gradient fill is iced-internal; verified manually during build.)

### 2. `src/config.rs` — config schema

Replace the empty struct with:

```rust
#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 2]
pub struct Config {
    pub sample_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { sample_interval_ms: 1500 }
    }
}
```

Drop `Default` from the derive list because we hand-implement it. `CosmicConfigEntry::get_entry` returns the field-by-field default for missing keys — so a v1 config file upgrading to v2 will pick up `1500 ms` automatically.

Bounds enforcement lives in `app.rs` (clamp to `[1000, 10000]` on read and on write); the schema itself is permissive.

### 3. `src/app.rs` — per-metric histories, popup, slider, subscription wiring

#### 3a. Constants

```rust
const PANEL_SPARK_SAMPLES: usize = 60;     // last-N window for the panel button
const HISTORY_CAP: usize = 900;            // ~15 min at 1 s polling (worst case)
const MIN_INTERVAL_MS: u64 = 1000;
const MAX_INTERVAL_MS: u64 = 10000;
```

`MAX_SAMPLES = 60` (`src/app.rs:22`) is replaced by these two: panel uses `PANEL_SPARK_SAMPLES`, every per-metric `VecDeque` is capped at `HISTORY_CAP`.

#### 3b. AppModel state

Add fields next to the existing `temp_history`:

```rust
config_handle:         Option<cosmic_config::Config>, // for write_entry
pending_interval_secs: Option<f32>,                   // live slider value during drag
temp_history:          VecDeque<f64>,                 // already exists
pump_speed_history:    VecDeque<f64>,                 // RPM as f64
pump_duty_history:     VecDeque<f64>,                 // %
fan_avg_speed_history: VecDeque<f64>,                 // mean RPM across fans
fan_avg_duty_history:  VecDeque<f64>,                 // mean % across fans
```

`pending_interval_secs` is `Some(v)` only while the user is mid-drag. The slider label and slider position both prefer this value when present, falling back to `self.config.sample_interval_ms / 1000.0` otherwise. **Only on slider release** is the value committed to `self.config.sample_interval_ms`, persisted via `write_entry`, and `pending_interval_secs` cleared. This keeps the subscription key (and thus the poll loop) stable during a drag — see 3e.

Reuse `fan_duty_avg(&[Fan])` (`src/app.rs:34-40`) and add a sibling `fan_speed_avg(&[Fan]) -> Option<u32>` with the same shape.

In `init` (`src/app.rs:97-108`), capture the handle:

```rust
let config_handle = cosmic_config::Config::new(APP_ID, Config::VERSION).ok();
let config = config_handle.as_ref().map(...).unwrap_or_default();
// ... then store both into AppModel via struct update syntax
```

#### 3c. Message enum

Add two new variants — one fires continuously during slider drag (cheap), the other fires once on release (commits + restarts subscription):

```rust
Message::SampleIntervalDragged(f32)  // continuous, every drag tick
Message::SampleIntervalReleased      // one-shot, on slider release
```

Handlers in `update`:

```rust
Message::SampleIntervalDragged(secs) => {
    self.pending_interval_secs = Some(secs);
}
Message::SampleIntervalReleased => {
    if let Some(secs) = self.pending_interval_secs.take() {
        let ms = (secs * 1000.0).round() as i64;
        let ms = ms.clamp(MIN_INTERVAL_MS as i64, MAX_INTERVAL_MS as i64) as u64;
        if ms != self.config.sample_interval_ms {
            self.config.sample_interval_ms = ms;
            if let Some(handle) = self.config_handle.as_ref() {
                let _ = self.config.write_entry(handle); // best-effort persist
            }
        }
    }
}
```

The subscription only sees the new interval after release because only `Released` mutates `self.config.sample_interval_ms` (which is what 3e keys on). During drag, only the transient `pending_interval_secs` changes — no subscription thrash.

#### 3d. StatusTick(Ok) handler

Append to all five histories (cap each at `HISTORY_CAP` with the existing `while pop_front` idiom). Compute fan averages once per tick before pushing. Stale-data preservation on `Err` is unchanged.

#### 3e. Subscription wiring (the load-bearing change)

Currently `Subscription::run_with("liquidctl-sub", ...)` (`src/app.rs:224`). Change the key to incorporate the configured interval and pass the interval into the closure:

```rust
let interval_ms = self.config.sample_interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
let key = format!("liquidctl-sub-{interval_ms}");
Subscription::run_with(key, move || {
    cosmic::iced::stream::channel(4, move |mut channel| async move {
        loop {
            // fetch, send, sleep
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
    })
})
```

When `interval_ms` changes, the subscription identity changes; iced tears down the old stream and starts a new one. This is the canonical iced pattern. The `move` closure captures the cloned `interval_ms`.

#### 3f. `view_window` — popup beautification

Replace the flat `list_column` (`src/app.rs:170-213`) with a `column![]` of cards/sections built from `widget::list_column()` + `widget::settings::section()`. Layout:

```text
┌─ Heading: device description ──────────────────┐
│                                                │
│  Coolant temperature                           │
│  ┌──────────────────────────────────────────┐  │
│  │   [320 × 80 sparkline w/ gradient]       │  │
│  └──────────────────────────────────────────┘  │
│  29.8 °C       (mono, large)                   │
│                                                │
│  Pump speed         [320 × 40 sparkline]       │
│  2640 rpm                                      │
│  Pump duty          [320 × 40 sparkline]       │
│  68 %                                          │
│                                                │
│  Fans (avg) speed   [320 × 40 sparkline]       │
│  1450 rpm                                      │
│  Fans (avg) duty    [320 × 40 sparkline]       │
│  62 %                                          │
│                                                │
│  Sample interval: 2.0 s                        │
│  ├──────────●──────────────────────────────┤   │
│                                                │
│  (optional error caption)                      │
└────────────────────────────────────────────────┘
```

- Heading via `widget::text::heading(status.description)` (existing convention).
- Each section uses `widget::text::caption(label)` for the metric name and `widget::text::body(value).font(cosmic::font::mono())` for the numeric value (matches the existing mono-on-numeric convention from `app.rs:179, 184, 193`).
- Sparklines are `Canvas::new(Sparkline::new(history.iter().copied())).width(Length::Fixed(320.0)).height(Length::Fixed(80.0))` for temp, `40.0` height for the four secondary metrics.
- Slider — note `.on_release()` is mandatory to avoid subscription thrash during drag (see 3c, 3e):

```rust
let secs = self
    .pending_interval_secs
    .unwrap_or((self.config.sample_interval_ms as f32) / 1000.0);
widget::slider(1.0..=10.0, secs, Message::SampleIntervalDragged)
    .step(0.5_f32)
    .on_release(Message::SampleIntervalReleased)
    .width(Length::Fill)
```

- Slider label uses `widget::text::body(format!("Sample interval: {secs:.1} s"))` and reads the same `secs` so it tracks the drag live.
- **Wrap the entire popup column in `widget::scrollable(...)`** so 5 sparklines + slider + labels never clip on narrower COSMIC panels. The scrollable becomes the popup root; everything else nests inside it.
- Existing `popup_settings.positioner.size_limits` (`src/app.rs:276-298`) needs widening: `min_width 320`, `max_width 380`, `min_height 360`, `max_height 1080`. With the scrollable wrapper, exceeding `max_height` degrades to scroll instead of clip.

#### 3g. `view` — panel button gradient

Update the panel `Sparkline::new(...)` site (`src/app.rs:129-131`) to feed only the most recent `PANEL_SPARK_SAMPLES` samples (slice off the tail of the `VecDeque`). The widget itself acquires the gradient in step 1 — no extra wiring needed.

---

## Existing utilities to reuse

| Where                                          | Reused for                                      |
| ---------------------------------------------- | ----------------------------------------------- |
| `y_range()` — `src/sparkline.rs:35`            | All five popup sparklines (auto-scale per metric).|
| `MIN_Y_SPAN = 2.0` — `src/sparkline.rs:16`     | Keep for temp; for non-temp metrics, the same floor is acceptable visually (sub-1-RPM jitter looks chaotic on a 40 px-tall canvas otherwise). |
| `fan_duty_avg()` — `src/app.rs:34-40`          | Pump-duty/fan-duty popup labels.                |
| `symbolic_icon()` — `src/app.rs:28-32`         | Reused if section icons are added (optional).   |
| `widget::list_column()` — `src/app.rs:170`     | Inner sections inside the popup column.         |
| `cosmic::font::mono()` — `src/app.rs:179`      | Numeric labels under each sparkline.            |
| `widget::text::heading/body/caption`           | Typographic hierarchy in popup.                 |
| Existing `temp_history` push-and-cap idiom — `src/app.rs:256-262` | Mirrored to all five buffers.   |

No new helper modules. The two new helpers are:

- `fn fan_speed_avg(&[Fan]) -> Option<u32>` next to `fan_duty_avg` (the same shape, summing `speed_rpm` instead).
- `Sparkline::with_stroke_alpha(self, alpha: f32) -> Self` builder method.

---

## Critical API references (verified)

- Slider: `cosmic::iced::widget::slider(start..=end, current, |v| Message::X).step(s).width(...)` — confirmed in `libcosmic@564ef83/examples/cosmic/src/window/demo.rs:295-299`. Re-exported from libcosmic at `widget::slider`.
- Linear gradient: `iced::widget::canvas::gradient::Linear::new(angle).add_stop(offset, color)` — `iced/core/src/gradient.rs:54-78`. `Linear` implements `Into<Fill>` via `Fill::from(Linear)` chain, so `frame.fill(&path, linear_gradient)` compiles.
- Path closing: `Path::new(|b| { b.move_to(...); b.line_to(...); ...; b.close(); })` — `iced/graphics/src/geometry/path/builder.rs:248`.
- Theme accent: `theme.cosmic().accent.base` — `cosmic-theme/src/model/theme.rs:59`. `theme: &Theme` is the third arg to `canvas::Program::draw`.
- Config write: `config.write_entry(&handle)` — `cosmic-config/src/lib.rs:444`. Requires keeping the `cosmic_config::Config` handle alive in `AppModel`.

---

## Verification

After implementing every step, in order:

1. **Build**: `just check` (clippy pedantic, `-D warnings`) → zero warnings, zero errors.
2. **Unit tests**: `cargo test` → existing 30 tests pass; new tests for `fan_speed_avg` and `Sparkline::with_stroke_alpha` pass.
3. **Visual smoke test** (requires hardware): `just run`. Confirm all of:
   - Panel sparkline shows a gradient under the polyline.
   - Click panel → popup opens with five sparklines, all rendering after the first poll.
   - Slider drag updates the "Sample interval: N.N s" label live; the poll cadence does NOT change mid-drag (visible by watching log output of `liquidctl` invocations); only on slider release does the cadence shift. Persists across applet restart.
   - At 1 s setting: popup sparklines fill out over ~15 min; at 10 s setting: poll cadence visibly slows, no UI jank.
   - Theme switch (Settings → Appearance → toggle dark/light) recolors the gradient on the next frame.
   - Popup with all 5 sparklines + slider scrolls if the COSMIC panel constrains its height; no content clipping.
4. **Stale-data**: pull the AIO USB cable for 5 s; popup keeps showing last sparkline data with an error caption (existing stale-preservation behavior must be intact).
5. **Config schema migration**: delete `~/.config/cosmic/com.github.cosmix.LiquidMon/v2/sample_interval_ms`, restart applet, slider lands on 1500 ms.
6. **Headless CI**: `cargo build --release` succeeds (no Wayland needed for compile).

---

## Knowledge update (final step)

After the change is verified, append entries to `doc/loom/knowledge/` (per the user's request and the project rule about preserving non-obvious findings). The agent doing the work edits these files directly with Write/Edit. Specifically:

- **`patterns.md`** — append a "Canvas Gradient-Filled Sparkline" section: how the area path is constructed (polyline + bottom corners + close), how the angle/stop convention was resolved at implementation time, why the stroke is drawn on top of the fill. Cite the new `src/sparkline.rs` line ranges.
- **`patterns.md`** — append "Theme-Accent Color in Canvas Programs": that `theme.cosmic().accent.base` is the canonical way to get the active accent inside a `canvas::Program::draw`, and that the type is `palette::Srgba` requiring conversion to `iced::Color`.
- **`patterns.md`** — append "Subscription Restart on Config Change": that including the configured value in the `Subscription::run_with` key is the canonical iced idiom for re-keying a stream when config changes.
- **`architecture.md`** — update the "AppModel State Structure" block with the new history buffers and `config_handle` field; update "Configuration System" to note the new field and version bump; update the "No Real Config Gap" section to reflect that the poll interval is no longer hardcoded.
- **`entry-points.md`** — update the "Notable Constants" table (replace `MAX_SAMPLES` with `PANEL_SPARK_SAMPLES`, `HISTORY_CAP`, `MIN_INTERVAL_MS`, `MAX_INTERVAL_MS`) and the "Where to Add New Features" table.
- **`concerns.md`** — mark the "Hardcoded polling interval" concern as RESOLVED with the date.
- **`mistakes.md`** — only if a non-trivial mistake surfaces during implementation (e.g. gradient angle inverted, stops out of order, popup size_limits clipping content). Format per the file's existing template.

---

## Out of scope

- Per-fan sparklines (only fan averages are tracked; an individual-fan view would multiply state and clutter the popup).
- Theme-aware per-metric color palette (decision: single accent across all sparklines for a coherent look).
- Configurable history depth (15 min is a reasonable fixed window).
- Migrating the device match filter (`"Hydro"`) to config — already tracked in `concerns.md`, separate change.
