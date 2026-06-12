# Concerns & Technical Debt

> Technical debt, warnings, issues, and improvements needed.
> This file is append-only - agents add discoveries, never delete.

## Hardcoded device match string — RESOLVED 2026-05-01

Originally `src/app.rs` passed the literal string `"Hydro"` to `fetch_status()` on every poll tick, locking the applet to Corsair Hydro descriptions and silently excluding every other liquidctl-supported AIO family.

**Resolved 2026-05-01:** Replaced with automatic enumeration plus a user-overridable dropdown:

- `Config` gained `device_match: Option<String>` (`#[version = 3]`, default `None`).
- New module `src/devices.rs` carries the lowercase substring catalog `AIO_PATTERNS` (currently `"hydro"` and `"icue h"` — families verified against the parser schema) plus pure helpers `is_aio`, `filter_aios`, and `auto_select`.
- New `liquidctl::list_devices() -> Vec<DetectedDevice>` enumerates connected devices (`liquidctl list --json`, 1 s timeout).
- `AppModel::effective_match()` resolves config-or-auto into the description sent to `liquidctl --match`. The poll subscription is keyed on `(interval_ms, match_str)` so changing the device tears down and restarts the loop. When no AIO is detected and no choice is saved, no poll subscription runs at all — preventing a spurious "no AIO" error during the brief startup-enumerate window.
- The popup dropdown shows `Auto (<description>)` plus all detected AIOs and a synthetic `<saved> (disconnected)` entry when the saved choice is offline.
- All `liquidctl` subprocess calls now serialize behind `LIQUIDCTL_LOCK: tokio::sync::Mutex<()>` to prevent the popup-open enumerate from racing with an in-flight poll on the same `/dev/hidrawN` claim.

Follow-ups deferred to `PLAN-aio-broad-support.md` (broader liquidctl families requiring parser changes) and a `--bus`/`--address` plan (truly-unique selection for two identical AIOs).

## Hardcoded polling interval — RESOLVED 2026-05-01

`src/app.rs:235` hard-codes `Duration::from_millis(1500)` as the poll interval. `Config` has no field for this. Users on slower machines or with USB hubs that throttle HID communication cannot tune this, and a user who only wants a 10-second refresh rate (to reduce USB traffic) has no way to set it.

**Resolved 2026-05-01:** Added `sample_interval_ms: u64` to `Config` (default 1500, `#[version = 2]`, hand-implemented `Default`). A slider in the popup exposes the range 1.0–10.0 s in 0.5 s steps. Drag events stage a transient `pending_interval_secs`; release commits and persists via `config.write_entry(&config_handle)`. The subscription re-keys on the clamped `interval_ms` value via `Subscription::run_with(interval_ms, fn_ptr)` — iced tears down and restarts the poll loop only when the committed value changes, keeping the running loop stable during a drag.

## `value.as_f64()` silently skips entries with non-numeric values — SUPERSEDED 2026-06-12

The silent-skip intent is now correctly implemented. See below.

## Fan index 0 rejection

`src/liquidctl.rs:219-221` explicitly returns `None` if the parsed fan index is `0`. This is a reasonable assumption for 1-based indexing, but `liquidctl` occasionally uses 0-based indexing for some controllers. Silently dropping `Fan 0` data without logging would be confusing to debug.

## No app-level tests for `app.rs::update` — partially resolved 2026-04-30, expanded 2026-05-01, further expanded 2026-06-12

`src/app.rs` now has 40 tests. `src/view.rs` adds 11 dropdown tests. `src/equalizer.rs` adds 10. `src/sparkline.rs` 9. `src/devices.rs` 6. `src/spinner.rs` 3. Total: 109.

Still untested: `view` / `view_window` rendering (trait-method entry points in `app.rs`), the `subscription` background task, the `TogglePopup` arm (depends on `core.main_window_id()` which requires a live Wayland surface), `fetch_status`'s subprocess invocation, `src/main.rs`, and `src/config.rs`. The first two need a headless iced/cosmic harness; subprocess testing would need a fake `liquidctl` binary on `PATH`.

## `tag` recipe in justfile uses `sed -i` with a fragile in-place substitution

`justfile` uses `find -type f -name Cargo.toml -exec sed -i '0,/^version/s/...'`. On macOS, `sed -i` requires an extension argument. Since this project targets Linux only, this is not a portability bug today, but it is worth noting as a platform assumption baked into tooling. The recipe now has a semver guard before any mutation (see "tag recipe does not validate version string format — RESOLVED" below).

## Subprocess command injection risk (low, but present)

`src/liquidctl.rs:116-118` constructs the command as `["--match", match_filter, "--json", "status"]`. If `match_filter` is later sourced from user-editable config, shell metacharacters are not a problem because `tokio::process::Command` does not invoke a shell. However, a filter containing `--` or `--json` could confuse `liquidctl`'s argument parser. Sanitizing or quoting the filter value before use would be prudent.

## No udev rule or installation documentation for non-root access

The `justfile` installs a binary and desktop entry but does not install a udev rules file granting the user read access to `/dev/hidraw*`. Without a udev rule, `liquidctl` requires `sudo`. Users who install via `just install` and do not separately run `scripts/install-liquidctl-udev.sh` will see only error states in the applet.

## `tag` recipe: `vendor` recipe uses `head -n -1` with `rm -rf`

`justfile:64-67` — the `vendor` recipe runs `cargo vendor ... | head -n -1 > .cargo/config.toml` and then `rm -rf .cargo vendor`. If the recipe is run in an existing checkout with a real `.cargo/config.toml`, it truncates that file before the `rm -rf` deletes the whole directory, losing any custom Cargo config. The recipe should create a temporary directory and only replace `.cargo` atomically after success.

## Platform assumption: Linux-only with no conditional compilation

The entire codebase assumes Linux. `src/liquidctl.rs:116` spawns `liquidctl` directly by name with no fallback for non-Linux platforms. There is no `#[cfg(target_os = "linux")]` gate. This is acceptable for the stated scope (Pop!_OS 24.04) but should be documented as an explicit constraint.

## `resources/app.metainfo.xml` missing udev dependency hint

The metainfo/AppStream file does not list a `requires` or `recommends` element for the `liquidctl` binary or a udev rules package. Package managers that parse AppStream data will not know to install `liquidctl` as a dependency.

## `StatusEntry.value` deserialization type mismatch defeats the silent-skip guard — RESOLVED 2026-06-12

`StatusEntry.value` is now typed as `serde_json::Value` (not `serde_json::Number`). A string/boolean/null value in a status entry now deserializes successfully and is silently skipped via the `as_f64()` guard, rather than failing the entire device parse with `Error::Parse`. The comment in `src/liquidctl.rs` documents the rationale. The silent-skip concern is resolved.

## Sparkline Y-axis hardcoded to 10–40°C — RESOLVED 2026-05-01

Originally `src/sparkline.rs:41-42` set `Y_MIN = 10.0` and `Y_MAX = 40.0`. Values outside this range silently mapped to off-canvas coordinates (the polyline disappeared entirely below 10 °C or above 40 °C — they did not "clamp to the edge" in the visual sense; they vanished). Additionally, the `samples.len() < 2` early-return left the sparkline blank for the first ~1.5 s after the first poll.

**Resolved** by replacing the static range with the pure helper `y_range(&[f64])` (`src/sparkline.rs:35`) which auto-scales from the sample window with a `MIN_Y_SPAN = 2.0` °C floor (centered on the data midpoint) so noise isn't amplified into apparent chaos. The `< 2` early-return was replaced with a single-sample horizontal-tick branch (`src/sparkline.rs:90-100`) so the sparkline is visible immediately after the first reading. Six unit tests cover the y_range helper.

Severity: was low-medium — now resolved.

## `fan_duty_avg` truncates integer division — RESOLVED 2026-06-12

`fan_duty_avg` and `fan_speed_avg` now round to nearest via `(sum + len/2) / len` rather than truncating. Both have `#[allow(clippy::cast_possible_truncation)]` with justifying in-code comments (the rounded mean of bounded values stays in range).

## `uninstall` recipe leaves metainfo file behind — RESOLVED 2026-06-12

`just uninstall` now removes `bin-dst`, `desktop-dst`, `appdata-dst`, and `icon-dst`. The install path is `share/metainfo/` (not the former `share/appdata/`) and the `uninstall` recipe removes `appdata-dst` which resolves to the new path.

## `tag` recipe does not validate version string format — RESOLVED 2026-06-12

`just tag <version>` now has a semver guard (`grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'`) at the top of the recipe before any Cargo.toml mutation. An invalid version string exits immediately with an error. The annotated tag message is `Release <version>` (not the bare version string).

## `libcosmic` pinned to a bare git SHA with no version tag

`Cargo.toml:17`: `rev = "564ef834cec33a948dc10c9b401cf29db5d18373"` pins libcosmic to a specific commit. `cargo update` cannot advance this automatically, so security fixes in libcosmic require a manual SHA update. The SHA carries no human-readable context (no tag, no comment indicating a release date or milestone). Fix: once libcosmic publishes semver releases on crates.io, migrate to a version constraint; in the interim, annotate the SHA with a comment indicating the date it was captured.

Severity: low — maintenance burden; no immediate security risk.

## Subscription channel buffer causes non-uniform poll timing under backpressure — PARTIALLY MITIGATED

The channel has a buffer of 4. With `tokio::time::interval + MissedTickBehavior::Delay` (added 2026-06-12) the tick-to-tick period is now independent of fetch duration. However, if the UI event loop falls behind (compositor suspended, high CPU load), the `channel.send().await` can still block the ticker for that iteration. The `Delay` policy means the NEXT tick is not burst-scheduled to compensate — it simply fires one full interval after the delayed send completes. Under sustained backpressure the effective sample rate degrades gracefully rather than erratically. Buffer of 4 still silently absorbs up to 4 queued ticks before applying send backpressure.

Severity: informational — acceptable for an applet; documented here for future reference.

## `vendor` recipe uses GNU-specific `head -n -1` — RESOLVED 2026-06-12

The `vendor` recipe now uses `mktemp` + `sed '$d'` (POSIX-safe last-line removal) instead of `head -n -1`. The recipe is also `bash`-explict (`#!/usr/bin/env bash`) with `set -euo pipefail`. No silent failure mode remains.

## No CI workflow files present — RESOLVED 2026-06-12

`.github/workflows/ci.yml` and `.github/workflows/release.yml` are present on disk. CI runs two jobs: `check` (fmt → desktop-file-validate → appstreamcli validate → clippy `-D warnings` → test → release build) and `audit` (cargo-audit). The earlier "no automated quality gate" concern is fully resolved.

## No canvas::Cache in popup equalizers — deferred (m14, 2026-06-12)

The three popup `Equalizer` canvases and the `Spinner` glyphs do not use `canvas::Cache`. They fully re-tessellate on every `AnimationTick` (~30 fps) while the popup is open. The load is bounded (popup-open only), GPU-composited by the COSMIC compositor, and has not produced measurable frame-drops. Addressing it requires a cross-cutting refactor touching `app.rs`, `equalizer.rs`, and `spinner.rs`. Deferred as marginal; revisit if users report GPU-related issues.

## TEMP_RANGE coolant ceiling at 55 °C may peg near danger zone — deferred (m15, 2026-06-12)

`view::TEMP_RANGE = (20.0, 55.0)` sets the equalizer's absolute hi to 55 °C, which is near the danger zone for many AIO coolers. At 52 °C the amber-to-red transition fires, which is the design intent (absolute, not auto-scaled). The numeric readout always shows the true value. Possible UX refinement: raise hi to 60–65 °C or introduce a distinct "pegged" visual; maintainer's call. Not a bug — intentional absolute-range design.

## VU ramp colors hardcoded at alpha 0.82 — unverified on light COSMIC themes (m16, 2026-06-12)

The equalizer LED cells use a fixed alpha 0.82 for lit cells. On dark COSMIC themes this renders well; on light themes the contrast against the popup background is unverified. A visual check on a light COSMIC theme is needed before declaring the color scheme theme-safe.

## AIO_PATTERNS has no false-positive guard for future non-AIO "hydro"-containing devices (m17, 2026-06-12)

`AIO_PATTERNS` includes `"hydro"` as a substring. There is no false-positive risk in the current liquidctl 1.16.0 catalog (98 devices), but a future non-AIO product using "hydro" in its description could be incorrectly classified as an AIO. Deferred; mitigated by the narrow pattern set and the fact that the dropdown lets users override auto-selection.

## Identical-AIO disambiguation and empty-string fallback in deserialize_string_lossy — deferred (m18/m19, 2026-06-12)

Two identical AIOs connected simultaneously are disambiguated by liquidctl's `--match` on description alone, which is ambiguous. `DetectedDevice` already carries `bus` and `address` for a future `--bus`/`--address` plan. The `deserialize_string_lossy` empty-string fallback for truly-unknown JSON types remains a best-effort safety net; both items deferred pending the `--bus`/`--address` work.

## resources/icon.svg is an empty 128×128 stub (m24, 2026-06-12)

`resources/icon.svg` has no path data. The `.deb` and `just install` both deploy it. `NoDisplay=true` limits user-facing exposure (the icon won't appear in app launchers), but the stub means software-center icon previews will be blank. A real icon is needed before wider distribution.

## Device-family list duplicated across Cargo.toml / README / metainfo (m28, 2026-06-12)

The list of supported AIO families appears in three places. While only one family is supported today, accepting this duplication knowingly; single-source-of-truth cleanup deferred until multi-family support lands.

## Clippy pedantic gate accuracy — important note for future reviewers (2026-06-12)

The ENFORCED quality gate is `cargo clippy --all-targets --all-features -- -D warnings` (default + correctness lints; pedantic NOT included). This is what CI runs and what `just ci-local` mirrors.

`just check` runs `-W clippy::pedantic` WITHOUT `--all-targets` — advisory, non-blocking. Under the full `cargo clippy --all-targets --all-features -- -W clippy::pedantic` command there are ~14 pre-existing pedantic warnings (e.g. `doc_markdown` on type names like `AioStatus`/`AIO_PATTERNS` in doc comments; `too_many_lines` on `update`; `cast_*` on the bounded cast helpers which use justifying in-code comments rather than `#[allow]`s; elidable lifetimes). These are intentionally unfixed; the project is NOT pedantic-clean.

**Rule for future reviewers:** never claim "zero pedantic warnings" without running the exact command `cargo clippy --all-targets --all-features -- -W clippy::pedantic` and reading all output. The `just check` output omits `--all-targets` and therefore misses warnings emitted only in test targets.
