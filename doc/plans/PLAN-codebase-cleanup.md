# Codebase Cleanup & Stabilization

## Context

`liquidmon` is a feature-complete COSMIC panel applet for Corsair AIO temperature, pump, and fan monitoring via the `liquidctl` CLI. Panel-button row of icon + temperature + sparkline + fan% + pump% wired to a 1500ms poll, with a list-column popup. The codebase still carries unused scaffolding from the System76 `cosmic-applet-template` and inconsistent naming across files, plus a handful of real reliability and packaging bugs.

This plan does four things:

1. **Removes** confirmed-unused scaffolding (i18n stack, dead `Config` field, dead patch comments, over-broad tokio features).
2. **Fixes** real reliability/correctness bugs (subprocess timeout, popup unwrap, error variant, float clamping).
3. **Names everything `liquidmon` / `com.github.cosmix.LiquidMon` consistently** across Cargo manifest, justfile, source code, desktop file, metainfo, README, and knowledge files.
4. **Rewrites** the boilerplate README and refreshes the loom knowledge files to match the code.

Locked-in decisions: MPL-2.0 license stays; i18n removed entirely; README rewritten from scratch; sparkline kept (shipped feature).

## Execution preamble: git initialization

There is no git repo here yet. Before any cleanup edits, the agent must initialize a fresh repo and commit a baseline of the current state so the cleanup commits land on top of a known starting point.

```bash
cd /home/dkaponis/src/liquidmon
git init
git branch -M main
# Stage explicit top-level entries — DO NOT use `git add -A` or `git add .`
git add .gitignore Cargo.toml Cargo.lock src resources scripts doc justfile i18n i18n.toml README.md
git commit -m "chore: initial baseline before liquidmon cleanup"
```

After this, the cleanup work proceeds as a sequence of logically grouped commits (see "Suggested commit sequence" in the kickoff). Skip `target/` (it's already in `.gitignore`).

## Confirmed unused — safe to remove

Cross-referenced via `rg` across `src/`:

| Item                                                                                  | Evidence                                                                 | Action                                                                                                                                                                                           |
| ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/i18n.rs` (53 lines)                                                              | `fl!()` macro never invoked outside `i18n.rs` itself                     | delete file                                                                                                                                                                                      |
| `i18n.toml`, `i18n/en/cosmic_liquid.ftl`, `i18n/` dir                                 | only consumer is `i18n.rs`                                               | delete                                                                                                                                                                                           |
| `i18n-embed`, `i18n-embed-fl`, `rust-embed` deps                                      | only used by `i18n.rs` and `main.rs:11` (`DesktopLanguageRequester`)     | `cargo remove` all three                                                                                                                                                                         |
| `mod i18n;` and `i18n::init(...)` in `src/main.rs:5,10-14`                            | wires the dead module                                                    | delete; main becomes ~6 lines                                                                                                                                                                    |
| `Config { demo: String }` (`src/config.rs:8`)                                         | `rg '\.demo\b\|self\.config\.'` returns zero matches                     | delete the `demo` field; keep the empty `Config` struct as-is so `cosmic_config` watch wiring keeps working as a no-op (cheap insurance for future settings) — **see "Empty Config risk" below** |
| Cargo.toml `[patch.'https://github.com/pop-os/libcosmic']` block (`Cargo.toml:40-44`) | commented-out, dev concluded                                             | delete the comment block                                                                                                                                                                         |
| `tokio = { features = ["full"] }`                                                     | only `tokio::process::Command` and `tokio::time::sleep` are used         | narrow to `features = ["process", "time"]`                                                                                                                                                       |
| Two commented-out `tracing::error!(...)` blocks (`src/app.rs:102-104, 243-245`)       | `tracing` is not a dependency; comments have shipped dead since template | delete the comments — see "Tracing decision" below                                                                                                                                               |

### Empty Config risk

`#[derive(CosmicConfigEntry)]` on a struct with **zero** fields is unverified. The derive macro iterates fields to generate getters/setters; it may either compile to no-op code (best case) or fail to compile (worst case). Three options:

1. **Most likely OK** — `pub struct Config {}` (named struct, braced empty body — **not** `pub struct Config;`, which is a unit struct and almost certainly will not satisfy the derive's field iteration). If `cargo build` succeeds, ship it.
2. **If derive rejects empty braced struct** — keep one private placeholder field of a derive-friendly type, e.g. `_reserved: u32` (no extra serde attrs needed; `CosmicConfigEntry` handles its own persistence). Comment the field as a placeholder until a real setting lands.
3. **Most aggressive** — if you're confident no settings will ever land, delete the entire `Config` struct, the `mod config;` declaration, the `config: Config` field on `AppModel`, the `cosmic_config::Config::new(...)` call in `init`, the `watch_config` subscription, and the `Message::UpdateConfig` variant. This collapses ~25 lines and is the most honest representation of "no settings."

Plan defaults to **path 1** with **path 2 as fallback**. Path 3 is a separate decision the user should make explicitly.

### Tracing decision

The two commented blocks in `init` and the config-watch subscription are the only places config errors would surface. Two reasonable directions:

- **Recommended**: delete the commented blocks. `cosmic_config` returns a partial config on error (`src/app.rs:101`), which is degradation-safe; with no real config fields, no operator action is possible anyway. Reintroduce later if real fields land.
- Alternative: add `tracing` + `tracing-subscriber` as deps, initialize in `main.rs`, restore both `tracing::error!` calls. Worth doing only if you want diagnostics in `journalctl` from `cosmic-panel` — not currently the case.

Plan adopts the **delete** path.

## Real bugs to fix

These survive into the shipped product. Concrete fixes:

### 1. `liquidctl` subprocess can hang the panel (`src/liquidctl.rs:108-128`)

`Command::output().await` has no timeout. A wedged USB device or hung `liquidctl` invocation freezes the poll loop forever; the panel button stops updating with no signal.

Fix: wrap the spawn in `tokio::time::timeout`, set `kill_on_drop(true)` on the `Command`, and add a new error variant. Requires adding `use std::time::Duration;` to the imports at `src/liquidctl.rs:5-8` (currently only `fmt`, `io`, and `tokio::process::Command` are imported).

```rust
// liquidctl.rs
use std::time::Duration;  // NEW import

pub enum Error {
    Spawn(io::Error),
    NonZeroExit { status: Option<i32>, stderr: String },
    Parse(serde_json::Error),
    NoDevice,
    MissingField(&'static str),  // see fix #3
    Timeout,                      // new
}

pub async fn fetch_status(match_filter: &str) -> Result<AioStatus, Error> {
    let mut cmd = Command::new("liquidctl");
    cmd.args(["--match", match_filter, "--json", "status"])
       .kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(3), cmd.output())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::Spawn)?;
    // ... rest unchanged
}
```

3-second timeout is comfortably above a healthy `liquidctl status` (~200-400ms on H150i Pro XT). The poll loop is strictly sequential (`await fetch_status` then `sleep 1500ms`), so multiple concurrent calls are impossible regardless of the timeout value — a wedged call simply delays the next tick by up to 3s.

### 2. `unwrap()` on `main_window_id()` (`src/app.rs:281`)

If `TogglePopup` fires before COSMIC assigns a main window (startup race), the applet panics. Fix:

```rust
let Some(parent) = self.core.main_window_id() else {
    self.popup = None;
    return Task::none();
};
let mut popup_settings = self.core.applet.get_popup_settings(
    parent, new_id, None, None, None,
);
```

### 3. `Error::NoDevice` is misleading for missing fields (`src/liquidctl.rs:173-175`)

When the device is found but lacks `Liquid temperature` / `Pump speed` / `Pump duty`, the user sees "no matching AIO device with usable status reported" — wrong message, hard to debug.

Fix: add `Error::MissingField(&'static str)` variant (see Error enum above) and replace the three `.ok_or(Error::NoDevice)?` with `.ok_or(Error::MissingField("liquid temperature"))?` etc.

Both new variants need `Display` arms appended at `src/liquidctl.rs:43-63`:

```rust
Error::Timeout => write!(f, "liquidctl call timed out"),
Error::MissingField(field) => write!(f, "device found but missing required status field: {field}"),
```

The `std::error::Error::source()` impl at `src/liquidctl.rs:65-73` needs no change — the `_ => None` arm already covers both new variants since neither carries an inner `dyn Error` source. The lossy `format!("{e}")` at `app.rs:229` automatically picks up the new Display messages, so no app-side changes are needed.

### 4. Unchecked `f64 → u8/u32` truncation (`src/liquidctl.rs:155-156, 162-163`)

Modern Rust (since 1.45) saturates float-to-int casts: `256.0_f64 as u8` yields `255`, not `0`, and a negative `f64` yields `0` rather than wrapping. So the casts are _defined_, not UB. The real concern is that saturating to `255` for a duty value silently masks an out-of-domain reading — a hardware-reported `256.0` should be flagged or clamped to the actual valid range (0-100% for duty), not quietly clipped to `255`. Explicit domain clamping makes the intended range visible at the cast site and catches sensor glitches more honestly:

```rust
let to_u8 = |v: f64| v.clamp(0.0, 255.0) as u8;
let to_u32 = |v: f64| v.clamp(0.0, u32::MAX as f64) as u32;
```

Apply at the four `as`-cast sites (`pump_speed`, `pump_duty`, fan `speed`, fan `duty`). Pump duty and fan duty are reported in 0-100% so `clamp(0.0, 100.0)` is even tighter.

### 5. README rewrite (`README.md`)

Existing 43-line file is template boilerplate: generic Fluent translator section, dead `[sccache]` link reference (line 43), nothing about the actual app. Replace with:

- One-paragraph what-and-why (Corsair H150i Pro XT, AIO temps + pump/fan duties, panel applet for COSMIC)
- Screenshot placeholder (or actual screenshot if available)
- Prerequisites: COSMIC desktop, `liquidctl` (Python package or distro pkg), HID udev rules — call out `scripts/install-liquidctl-udev.sh` explicitly (currently undocumented; closes the "no udev docs" concern)
- Install: `just build-release && sudo just install`
- Supported devices: any whose `liquidctl status` description contains "Hydro" (current hardcoded match filter)
- Troubleshooting: panel shows `!` → `liquidctl --match Hydro --json status` from terminal; permission errors → re-run udev script
- Development: `just run`, `just check`, `cargo test`
- License: MPL-2.0

Drop the entire Translators section (no i18n) and the dead sccache link.

## Naming

Final identifiers everywhere in the project:

| What            | Value                                 |
| --------------- | ------------------------------------- |
| Cargo package   | `liquidmon`                           |
| Binary          | `liquidmon`                           |
| RDNN / APP_ID   | `com.github.cosmix.LiquidMon`         |
| GitHub repo URL | `https://github.com/cosmix/liquidmon` |
| Display name    | `LiquidMon`                           |

Files where these values must appear:

| File                         | Edit                                                                                                                                                                                                                                                                                                                                                        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`                 | `name = "liquidmon"`, `repository = "https://github.com/cosmix/liquidmon"`                                                                                                                                                                                                                                                                                  |
| `justfile:1-2`               | `name := 'liquidmon'`, `appid := 'com.github.cosmix.LiquidMon'`                                                                                                                                                                                                                                                                                             |
| `src/app.rs:80`              | `const APP_ID: &'static str = "com.github.cosmix.LiquidMon";`                                                                                                                                                                                                                                                                                               |
| `resources/app.desktop:5,6`  | `Icon=com.github.cosmix.LiquidMon`, `Exec=liquidmon %F`                                                                                                                                                                                                                                                                                                     |
| `resources/app.metainfo.xml` | `<id>`, `<launchable>`, `<provides><id>` → `com.github.cosmix.LiquidMon`; `<provides><binary>` → `liquidmon`; both URLs to `https://github.com/cosmix/liquidmon`; **drop the `<icon type="remote">` element entirely** (its current path `resources/icons/hicolor/scalable/apps/icon.svg` does not exist; installed icon resolves via the AppStream `<id>`) |
| `README.md`                  | use `liquidmon` and `LiquidMon` consistently (covered by the rewrite in fix #5)                                                                                                                                                                                                                                                                             |
| `doc/loom/knowledge/*.md`    | use `liquidmon` / `LiquidMon` / `com.github.cosmix.LiquidMon` / `https://github.com/cosmix/liquidmon` consistently (covered by the knowledge refresh)                                                                                                                                                                                                       |

After the edits, `rg -n -i 'cosmic.liquid|CosmicLiquid|com\.github\.dkaponis|github\.com/dkaponis' .` should return zero hits anywhere in the tree.

## Build & packaging hygiene

These are smaller items from `concerns.md` that should travel with the cleanup commit:

- **Pin `libcosmic` to a known-good rev** (`Cargo.toml:22`). With dev concluded, `git = "..."` with no `rev =` makes every fresh `cargo fetch` a roulette. Concrete action — current `Cargo.lock` resolves libcosmic to `git+https://github.com/pop-os/libcosmic.git#564ef834cec33a948dc10c9b401cf29db5d18373`, so update the dependency block to:

  ```toml
  [dependencies.libcosmic]
  git = "https://github.com/pop-os/libcosmic.git"
  rev = "564ef834cec33a948dc10c9b401cf29db5d18373"
  features = [...]  # unchanged
  ```

  No `cargo update` should be needed if the `rev` SHA matches what `Cargo.lock` already resolved (it does — that's how we got the SHA). If a dry-run `cargo build` complains about a lockfile-source mismatch, run `cargo update -p libcosmic` to refresh; otherwise skip it (avoids an unnecessary network fetch).

- **`justfile` `vendor` recipe is incomplete**, not just destructive (`justfile:62-67`). Three issues:
  1. It never produces `vendor.tar`, but `vendor-extract` (the consumer at `justfile:70-72`) expects one. The missing step is `tar pcf vendor.tar vendor .cargo` between the `echo` lines and the `rm -rf`.
  2. **Critical**: `vendor-extract` only does `tar pxf vendor.tar`; it does not regenerate `.cargo/config.toml`. So `vendor.tar` MUST contain both `vendor/` and `.cargo/config.toml`, otherwise `cargo build --frozen --offline` in a fresh checkout has no clue where the vendored sources live and fails.
  3. The `cargo vendor ... > .cargo/config.toml` form will clobber a user's existing `.cargo/config.toml`.

  Corrected recipe:

  ```text
  vendor:
      mkdir -p .cargo
      cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
      echo 'directory = "vendor"' >> .cargo/config.toml
      echo >> .cargo/config.toml
      tar pcf vendor.tar vendor .cargo
      rm -rf .cargo vendor
  ```

  Including `.cargo` in the tarball means `vendor-extract` restores the cargo redirection alongside the vendor dir — `--frozen --offline` then works end-to-end. Risk to a user's pre-existing `.cargo/config.toml` is real but mitigated by the fact that this recipe is only intended for packaging contexts where there is no pre-existing config — leaving as-is is acceptable, or a paranoid version stages into a temp dir first.

- **`metainfo.xml` `<recommends>liquidctl</recommends>` — DROPPED from this plan.** AppStream's `<recommends>` element wraps a child like `<id>` (component ID), `<modalias>`, `<kernel>`, `<memory>`, `<firmware>`, etc. The `liquidctl` Python project does not publish an AppStream component (it is a CLI, not a desktop app), so `<id>liquidctl</id>` would not resolve and AppStream validators would warn. The honest place to communicate the runtime dependency on `liquidctl` is the README's prerequisites section (already covered) and any distro packaging files, not metainfo.

## Knowledge file refresh & additions (post-cleanup)

This is **part of the cleanup commit, not a follow-up**. Knowledge files describe the system as it is _after_ the changes land. Three categories of work:

### A. Update — sync stale references

`doc/loom/knowledge/architecture.md`, `entry-points.md`, and `patterns.md` are all stale — they describe `app.rs` at 251 lines with a single text label panel button; today it is 308 lines with a full row of icons + sparkline + temp_history. Line refs for subscription, popup toggle, message handling are all off by 50+ lines.

- Update `architecture.md` panel-button section (lines 148-152) to describe the row layout, sparkline, and `temp_history: VecDeque<f64>` capped at `MAX_SAMPLES = 60`. Also update the `Cross-Cutting Concerns Synthesis` block to reflect that the timeout, unwrap, MissingField, and clamping concerns are now resolved.
- Update `entry-points.md` line counts (current `app.rs` size, `liquidctl.rs` size after the new variants/imports) and add a "Sparkline" path entry (`src/sparkline.rs:1-72`, used at `app.rs:135`). Also add the three embedded SVG icon constants and `AUTOSIZE_ID`.
- Update `patterns.md` line refs throughout — Subscription pattern (currently cites `app.rs:166-196`, real location is `app.rs:219-249`), stale-data preservation comment (cites `app.rs:212-215`, now `270-272`), popup toggle idiom (cites `app.rs:218-236`, now `274-294`), and the panel-view pattern section (currently describes a single text widget, today is a row layout).
- Update `stack.md` to reflect: tokio features narrowed to `["process", "time"]`; libcosmic pinned at rev `564ef83…`; `i18n-embed`, `i18n-embed-fl`, `rust-embed` removed; the `i18n.toml` and `i18n/` directory deleted.
- Update `conventions.md` if any conventions changed (e.g., the Error enum now has `MissingField(&'static str)` carrying a static string field name — note the convention).

### B. Remove — delete content describing now-deleted code

- Remove the i18n sections from `architecture.md` (lines 161-162, 196-198) and `stack.md` (lines 60-69, 112-120) — those describe deleted code.
- Remove the now-resolved entries from `concerns.md`: timeout, unwrap, error variant, casts, README sccache, libcosmic floating pin, dead `demo` field, vendor recipe. Leave entries that remain unresolved (hardcoded match filter, hardcoded poll interval, `value.as_f64()` silent skip, fan index 0 rejection, no app-level tests, sed-based `tag` recipe).
- Apply four mechanical replacements across all knowledge files (architecture.md, entry-points.md, stack.md, patterns.md, concerns.md, conventions.md, mistakes.md):
  1. `s/cosmic-liquid/liquidmon/g` (file paths, package/binary name)
  2. `s/CosmicLiquid/LiquidMon/g` (RDNN suffix and display contexts)
  3. `s/com\.github\.dkaponis/com.github.cosmix/g` (RDNN third component, also updates GitHub URL paths)

  After running these, `rg -n -i 'cosmic.liquid|CosmicLiquid|com\.github\.dkaponis|github\.com/dkaponis' doc/loom/knowledge/` should return zero hits.

### C. Add — capture new knowledge gained from this work

- **In `mistakes.md`**, record any **execution-time mistakes** encountered while doing this cleanup. Examples: if the empty `Config {}` derive path failed and the `_reserved` placeholder was needed, record the rule (`CosmicConfigEntry` does not support empty braced structs). If the libcosmic `rev` pin caused a version-resolution conflict, record that. Use the format:

  ```markdown
  ## [Short description]

  **What happened:** [What went wrong]
  **Why:** [Root cause]
  **Prevention:** [How to detect earlier]
  **Fix:** [What to do instead]
  ```

  Skip if no mistakes occurred. Do **not** add a generic "scaffolding cruft" rule — that's covered by the cleanup itself, not a future-prevention lesson.

- **In `patterns.md`**, add a new pattern entry under "Error Handling Patterns" describing the bounded-timeout-with-kill-on-drop pattern used in `fetch_status` (the `tokio::time::timeout(...) + .kill_on_drop(true)` combo). Cite the file and line.

- **In `concerns.md`**, _do not_ add new concerns from this cleanup unless something genuinely surprising came up — resolved entries are simply deleted, not annotated.

### D. Verify — confirm knowledge files are coherent

- After updates, `rg -n -i 'cosmic.liquid|CosmicLiquid' doc/loom/knowledge/` returns zero hits.
- After updates, `rg -n 'i18n|fl!|FluentLanguageLoader' doc/loom/knowledge/` returns zero hits.
- Spot-check at least three line refs in `patterns.md` and `entry-points.md` against actual code: open the cited line in `src/app.rs` and verify the description matches.
- Read `architecture.md` start-to-finish and confirm it describes the post-cleanup state (no stale references, no contradictions with the actual code).

## Critical files modified

| File                                                                                                                               | Change                                                                                                                                                                                                                                                                                              |
| ---------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/main.rs`                                                                                                                      | Remove `mod i18n;` and i18n init — collapses to ~9 lines                                                                                                                                                                                                                                            |
| `src/i18n.rs`                                                                                                                      | **DELETE**                                                                                                                                                                                                                                                                                          |
| `i18n.toml`                                                                                                                        | **DELETE**                                                                                                                                                                                                                                                                                          |
| `i18n/en/cosmic_liquid.ftl`, `i18n/` dir                                                                                           | **DELETE**                                                                                                                                                                                                                                                                                          |
| `src/config.rs`                                                                                                                    | Remove dead `demo: String` field                                                                                                                                                                                                                                                                    |
| `src/app.rs`                                                                                                                       | Replace `unwrap()` at line 281; remove two commented `tracing::error!` blocks; update `APP_ID` const to `LiquidMon`                                                                                                                                                                                 |
| `src/liquidctl.rs`                                                                                                                 | Add `Timeout` + `MissingField` variants; wrap `output()` in `tokio::time::timeout` with `kill_on_drop`; replace casts with clamping                                                                                                                                                                 |
| `Cargo.toml`                                                                                                                       | Remove `i18n-embed`, `i18n-embed-fl`, `rust-embed`; narrow tokio features; pin libcosmic rev; remove commented patch block; rename `name` and `repository` to `liquidmon`                                                                                                                           |
| `Cargo.lock`                                                                                                                       | Updated by `cargo remove`, `cargo update -p libcosmic`, and the package rename                                                                                                                                                                                                                      |
| `README.md`                                                                                                                        | Full rewrite (uses `liquidmon`)                                                                                                                                                                                                                                                                     |
| `justfile`                                                                                                                         | Add missing `tar pcf vendor.tar vendor .cargo` step in `vendor` recipe (must include `.cargo` so `vendor-extract` restores the cargo redirection); rename `name` and `appid` vars to `liquidmon` / `LiquidMon`                                                                                      |
| `resources/app.desktop`                                                                                                            | Update `Icon=` and `Exec=` to `LiquidMon` / `liquidmon`                                                                                                                                                                                                                                             |
| `resources/app.metainfo.xml`                                                                                                       | Update `<id>`, `<launchable>`, `<provides><id>`, `<provides><binary>`, and both URLs to `liquidmon` / `LiquidMon`; **fix or drop the broken `<icon type="remote">` element** (current path `resources/icons/hicolor/scalable/apps/icon.svg` does not exist — see rename section for resolution)     |
| `doc/loom/knowledge/architecture.md`, `entry-points.md`, `stack.md`, `patterns.md`, `concerns.md`, `conventions.md`, `mistakes.md` | Sync with new code (line refs, sparkline, panel row layout); apply the three mechanical replacements per "Knowledge file refresh & additions" section B (cosmic-liquid → liquidmon, CosmicLiquid → LiquidMon, com.github.dkaponis → com.github.cosmix) |

## Verification

End-to-end checks after the changes land. Run from repo root unless noted.

1. **Compile clean**: `cargo build --release` — no errors, no new warnings.
2. **Lint clean**: `just check` — no clippy regressions vs current baseline.
3. **Unit tests pass**: `cargo test` — existing three tests in `liquidctl.rs:213-267` still pass; add at least one test for `Error::MissingField` (e.g., a fixture with the device but missing `"Liquid temperature"` should now return `Err(Error::MissingField("liquid temperature"))` instead of `Err(Error::NoDevice)`). No test for the timeout path — `fetch_status` spawns a real subprocess and isn't easily mockable; rely on tokio's well-tested `timeout` primitive.
4. **Vendored build smoke**: `just vendor && just build-vendored` — confirms the corrected `vendor` recipe produces `vendor.tar` and the offline build consumes it. Run in a clean checkout so a stray local `.cargo/config.toml` doesn't get clobbered.
5. **Run the applet**: `just run` — panel shows the icon-row layout; sparkline animates as temp_history fills (up to 60 samples = 90s of history).
6. **Manual unwrap test**: hard to repro the startup race, but verify the new `let Some(parent) = ...` path compiles and types check; visually confirm popup still opens after a normal startup.
7. **i18n removal smoke**: `rg -n 'i18n|fl!|FluentLanguageLoader|DesktopLanguageRequester' src/ Cargo.toml` should return zero hits. (Note: `Cargo.lock` will still contain `i18n-embed` and `i18n-embed-fl` entries because libcosmic depends on them transitively; that's expected and not a regression. Only direct app-level references in `src/` and `Cargo.toml` need to vanish.)
   7a. **Naming consistency smoke**: `rg -n -i 'cosmic.liquid|CosmicLiquid|com\.github\.dkaponis|github\.com/dkaponis' src/ Cargo.toml Cargo.lock justfile resources/ README.md doc/loom/knowledge/` should return zero hits. The Cargo.lock root package entry should be `name = "liquidmon"`. The binary at `target/release/liquidmon` should exist after `cargo build --release`.
8. **Knowledge files re-read** by Claude in next session — verify they describe the post-cleanup state correctly (no references to deleted i18n code, correct line counts, sparkline documented).
9. **README review**: render the new README locally (`gh markdown` or any markdown viewer) and confirm no dead links and the udev step is callable end-to-end on a fresh machine.

## Out of scope (deferred)

- **Hardcoded `"Hydro"` filter and 1500ms poll interval** — flagged in `concerns.md` as "should be config-driven." With dev concluded and only one device on one machine, leave as constants. If/when a second device or a slower-poll preference appears, revisit Config.
- **Adding tests for `app.rs::update`** — flagged in concerns.md. The `Message::TogglePopup` arm pulls in `cosmic::iced` types that are awkward to mock; the `StatusTick(Ok)` arm with `temp_history` push/pop is testable but low-value relative to a manual end-to-end run. Skipping unless a regression motivates it.
- **`sed -i` portability in `justfile:76` `tag` recipe** — Linux-only project; not a real bug.
