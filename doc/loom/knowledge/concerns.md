# Concerns & Technical Debt

> Technical debt, warnings, issues, and improvements needed.
> This file is append-only - agents add discoveries, never delete.

## Hardcoded device match string

`src/app.rs:229` passes the literal string `"Hydro"` to `fetch_status()` on every poll tick. This is not derived from the `Config` struct and cannot be changed by the user without recompiling. The intended fix is to add a `device_filter: String` field to `Config` and thread it through the subscription. The match filter is a positional argument to `liquidctl --match`, so any device whose description does not contain `"Hydro"` will never be detected.

## Hardcoded polling interval

`src/app.rs:235` hard-codes `Duration::from_millis(1500)` as the poll interval. `Config` has no field for this. Users on slower machines or with USB hubs that throttle HID communication cannot tune this, and a user who only wants a 10-second refresh rate (to reduce USB traffic) has no way to set it.

## `value.as_f64()` silently skips entries with non-numeric values

`src/liquidctl.rs:162-164` — if `liquidctl` ever emits a status entry with a string or boolean value (e.g. firmware version, firmware checksum), `entry.value.as_f64()` returns `None` and the key is silently skipped. This is acceptable for truly non-numeric keys but masks unexpected numeric entries that fail to parse as f64.

## Fan index 0 rejection

`src/liquidctl.rs:219-221` explicitly returns `None` if the parsed fan index is `0`. This is a reasonable assumption for 1-based indexing, but `liquidctl` occasionally uses 0-based indexing for some controllers. Silently dropping `Fan 0` data without logging would be confusing to debug.

## No app-level tests for `app.rs::update` — partially resolved 2026-04-30

`src/app.rs` now has a `#[cfg(test)] mod tests` block covering `fan_duty_avg` and the `StatusTick(Ok)` / `StatusTick(Err)` / `PopupClosed` / `UpdateConfig` arms of `update`, plus the `MAX_SAMPLES` cap on `temp_history` (10 tests). The test module imports `cosmic::Application as _` to expose the trait method and constructs the model via `AppModel::default()`.

Still untested: `view` / `view_window` rendering, the `subscription` background task, the `TogglePopup` arm (depends on `core.main_window_id()` which requires a live Wayland surface), `fetch_status`'s subprocess invocation, `src/main.rs`, and `src/config.rs`. The first two need a headless iced/cosmic harness; subprocess testing would need a fake `liquidctl` binary on `PATH`.

## `tag` recipe in justfile uses `sed -i` with a fragile in-place substitution

`justfile:76` uses `find -type f -name Cargo.toml -exec sed -i '0,/^version/s/...'`. On macOS, `sed -i` requires an extension argument. Since this project targets Linux only, this is not a portability bug today, but it is worth noting as a platform assumption baked into tooling.

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

## `StatusEntry.value` deserialization type mismatch defeats the silent-skip guard

`src/liquidctl.rs:107` types `StatusEntry.value` as `serde_json::Number`. The intent of `entry.value.as_f64()` returning `None` (lines 162-164) is to silently skip non-numeric entries. However, `serde_json::Number` only deserializes successfully if the JSON token is a number. If `liquidctl` emits a status entry whose value is a JSON string or boolean (e.g. firmware version as `"1.0.2"`), `serde_json::from_str` at line 143 will return `Error::Parse` for the entire device, not `None` for that entry. The silent-skip behaviour at line 162 is therefore unreachable for non-numeric value types. Fix: type `value` as `serde_json::Value` and then call `.as_f64()` on it.

Severity: medium — may cause complete parse failure when a future liquidctl version adds string-typed status fields.

## Sparkline Y-axis hardcoded to 10–40°C

`src/sparkline.rs:41-42` sets `Y_MIN = 10.0` and `Y_MAX = 40.0`. Values outside this range silently clamp to the top or bottom edge. High-load scenarios or poorly cooled systems can push liquid temps above 40°C; the sparkline would show a flat top line with no visual distinction between 40°C and 55°C, masking a thermal alert. The range should either be dynamic (min/max of the sample window) or configurable, and the user should receive some out-of-range indication.

Severity: low-medium — cosmetic but misleading under thermal stress.

## `fan_duty_avg` truncates integer division

`src/app.rs:39`: `sum / fans.len() as u32` performs integer division after summing `u32` duty percentages. Fractional results are truncated silently. For example, two fans at 41% and 40% yields 40% rather than 40.5% (displayed as 40%). The discrepancy is small but grows with more fans at uneven duties. Fix: compute as `f64` and round, or use `(sum + fans.len() as u32 / 2) / fans.len() as u32` for rounding.

Severity: low — cosmetic rounding error.

## `uninstall` recipe leaves metainfo file behind

`justfile:58-60`: `just uninstall` removes `bin-dst`, `desktop-dst`, and `icon-dst` but does not remove `appdata-dst` (`/usr/share/appdata/com.github.cosmix.LiquidMon.metainfo.xml`). AppStream/software-center tooling may continue listing the application after uninstall. Fix: add `rm {{appdata-dst}}` to the uninstall recipe.

Severity: low — packaging hygiene.

## `tag` recipe does not validate version string format

`justfile:76-82`: `just tag <version>` writes the provided string directly into Cargo.toml via `sed` and creates a git tag. There is no validation that `<version>` is valid semver (e.g., `just tag foo-bar` would mutate Cargo.toml before `cargo check` catches the invalid version). The git commit and Cargo.toml modification happen before `cargo check`. Fix: add a `grep -qP '^\d+\.\d+\.\d+' <<< "{{version}}"` guard at the top of the recipe, or use a dedicated version-bump tool.

Severity: low — developer workflow hazard.

## `libcosmic` pinned to a bare git SHA with no version tag

`Cargo.toml:17`: `rev = "564ef834cec33a948dc10c9b401cf29db5d18373"` pins libcosmic to a specific commit. `cargo update` cannot advance this automatically, so security fixes in libcosmic require a manual SHA update. The SHA carries no human-readable context (no tag, no comment indicating a release date or milestone). Fix: once libcosmic publishes semver releases on crates.io, migrate to a version constraint; in the interim, annotate the SHA with a comment indicating the date it was captured.

Severity: low — maintenance burden; no immediate security risk.

## Subscription channel buffer causes non-uniform poll timing under backpressure

`src/app.rs:231`: the channel created by `cosmic::iced::stream::channel(4, ...)` has a buffer of 4. The async loop at lines 234-242 sends a message then sleeps 1500 ms. If the UI event loop falls behind (compositor suspended, high CPU load), the send at line 238 will `.await` until the receiver drains a slot — blocking the sleep timer and causing the effective poll interval to expand unpredictably. A buffer of 1 would make the back-pressure visible immediately; a buffer of 4 silently absorbs up to 6 seconds of stale readings. This is unlikely to matter in practice but should be documented.

Severity: informational — timing behaviour under backpressure is not documented.

## `vendor` recipe uses GNU-specific `head -n -1`

`justfile:64`: `cargo vendor ... | head -n -1 > .cargo/config.toml` relies on GNU `head`'s negative-count extension. On BSD/macOS `head`, `-n -1` is an error. Even on Linux, if `cargo vendor` emits zero lines, `head -n -1` produces empty output, silently writing an empty `.cargo/config.toml`. Fix: use `cargo vendor ... > /tmp/vendor-config.toml && sed '$d' /tmp/vendor-config.toml > .cargo/config.toml` (POSIX-safe last-line removal) or parse the config with a purpose-built tool.

Severity: low — affects Linux-only tooling but creates a silent failure mode.

## No CI workflow files present

`.github/workflows/` does not exist in the repository. The git log references `ci.yml` and `release.yml` in a past commit (`6f9b43b ci: add lint/test/build and tag-driven release workflows`), but these files are absent from the working tree. Either they were deleted or were never committed. Without CI, there is no automated gate on `cargo clippy`, `cargo test`, or release artifact builds. Any regression in compilation or test coverage goes undetected until a developer manually runs `just check`.

Severity: medium — no automated quality gate exists.
