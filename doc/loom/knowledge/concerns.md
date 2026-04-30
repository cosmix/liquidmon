# Concerns & Technical Debt

> Technical debt, warnings, issues, and improvements needed.
> This file is append-only - agents add discoveries, never delete.

(Add concerns as you discover them)

## Hardcoded device match string

`src/app.rs:174` passes the literal string `"Hydro"` to `fetch_status()` on every poll tick. This is not derived from the `Config` struct and cannot be changed by the user without recompiling. The intended fix is to add a `device_filter: String` field to `Config` and thread it through the subscription. The match filter is a positional argument to `liquidctl --match`, so any device whose description does not contain `"Hydro"` will never be detected.

## Hardcoded polling interval

`src/app.rs:180` hard-codes `Duration::from_millis(1500)` as the poll interval. `Config` has no field for this. Users on slower machines or with USB hubs that throttle HID communication cannot tune this, and a user who only wants a 10-second refresh rate (to reduce USB traffic) has no way to set it.

## Config struct carries no real configuration

`src/config.rs:8` defines `Config` with a single `demo: String` field that is never read or written anywhere in `app.rs`. The struct is loaded and watched for changes (`src/app.rs:68-79`, `src/app.rs:187-195`) but `UpdateConfig` does nothing with the new value. This is leftover scaffolding — the config version is `1` but no meaningful fields exist yet.

## Commented-out error logging (config errors silently discarded)

`src/app.rs:71-76` and `src/app.rs:190-193` both contain identical commented-out `tracing::error\!` calls. Config load errors (line 71-76) and config watch errors (line 190-193) are silently swallowed. Until the tracing calls are restored, operator-visible diagnostics for configuration problems are absent.

## `unwrap()` on main window ID

`src/app.rs:224` calls `self.core.main_window_id().unwrap()`. If the COSMIC runtime does not assign a main window before the first `TogglePopup` message fires (possible during startup), this panics and kills the applet process. A `?`-propagating or graceful-fallback approach is needed.

## No timeout on `liquidctl` subprocess

`src/liquidctl.rs:109-113` — `Command::output().await` waits indefinitely for `liquidctl` to finish. If the `liquidctl` process hangs (e.g. the USB device is mid-reset or udev is slow), the subscription loop stalls forever and the panel button stops updating without any user-visible indication. A `tokio::time::timeout` wrapper is needed around the `.output()` call.

## No liquidctl process kill on timeout / retry

Related to the above: if a timeout were added, the child process would need to be explicitly killed. `tokio::process::Command::kill_on_drop` is not set, so a timed-out child would become a zombie until the next poll cycle reaps it.

## Subprocess command injection risk (low, but present)

`src/liquidctl.rs:109-113` constructs the command as `["--match", match_filter, "--json", "status"]`. If `match_filter` is later sourced from user-editable config, shell metacharacters are not a problem because `tokio::process::Command` does not invoke a shell. However, a filter containing `--` or `--json` could confuse `liquidctl`'s argument parser. Sanitizing or quoting the filter value before use would be prudent.

## `value.as_f64()` silently skips entries with non-numeric values

`src/liquidctl.rs:149-151` — if `liquidctl` ever emits a status entry with a string or boolean value (e.g. firmware version, firmware checksum), `entry.value.as_f64()` returns `None` and the key is silently skipped. This is acceptable for truly non-numeric keys but masks unexpected numeric entries that fail to parse as f64.

## Unchecked f64-to-u8/u32 truncation

`src/liquidctl.rs:155-156, 162-163` cast `value_f64 as u32` and `value_f64 as u8`. In Rust, casting an out-of-range float to an integer is defined (saturating on overflow) but produces wrong data silently. A pump duty of `256.0` would display as `0`. Using `value_f64 as u32` when the value could realistically be negative (e.g. if liquidctl reports `−1` as an error sentinel) would wrap to `u32::MAX`.

## Fan index overflow

`src/liquidctl.rs:207` parses the fan index as `u8` via `.parse().ok()?`. A device reporting `Fan 256 speed` would cause `.parse::<u8>()` to return `None`, silently dropping that fan entry. Fan indices above 255 are unrealistic today but the silent discard is worth noting.

## `Error::NoDevice` reused for missing required fields

`src/liquidctl.rs:173-175` — if `Liquid temperature`, `Pump speed`, or `Pump duty` keys are absent from the status array, the error returned is `Error::NoDevice`. This message ("no matching AIO device with usable status reported") is misleading when the device was found but just missing a specific field. A separate `Error::MissingField(String)` variant would give clearer diagnostics.

## `libcosmic` pinned to floating `git` HEAD

`Cargo.toml:22` — `libcosmic` is a `git =` dependency with no `rev =` or `tag =` pin. Every fresh `cargo fetch` or `cargo build` may pull a different commit. This makes builds non-reproducible and can silently introduce breaking changes. It also means the vendored tarball (`just vendor`) must be regenerated whenever the upstream API changes. A `rev =` pin to a known-good commit is the standard practice for git dependencies.

## `tokio = { features = ["full"] }` — over-broad feature selection

`Cargo.toml:15` enables all tokio features including `fs`, `net`, `signal`, `process`, `io-std`, etc. Only `time` and `process` are actually used. The bloat is minor in a GUI app but signals that the dependency was not thoughtfully scoped.

## No unit tests outside `liquidctl.rs`

`src/main.rs`, `src/app.rs`, and `src/config.rs` have no `#[cfg(test)]` modules. The UI logic (`view`, `view_window`, `update`) is untested. At minimum, the `update` method's state transitions (`StatusTick(Ok)`, `StatusTick(Err)`, `PopupClosed`) should have unit tests.

## `split_fan_key` rejects fan index 0

`src/liquidctl.rs:207-209` explicitly returns `None` if the parsed fan index is `0`. This is a reasonable assumption for 1-based indexing, but `liquidctl` occasionally uses 0-based indexing for some controllers. Silently dropping `Fan 0` data without logging would be confusing to debug.

## No udev rule or installation documentation for non-root access

The `justfile` installs a binary and desktop entry but does not install a udev rules file granting the user read access to `/dev/hidraw*`. Without a udev rule, `liquidctl` requires `sudo`. The README does not mention this requirement. Users who install via `just install` will see only error states in the applet.

## `vendor` recipe in justfile uses `head -n -1` with `rm -rf`

`justfile:64-67` — the `vendor` recipe runs `cargo vendor ... | head -n -1 > .cargo/config.toml` and then `rm -rf .cargo vendor`. If the recipe is run in an existing checkout with a real `.cargo/config.toml`, it truncates that file before the `rm -rf` deletes the whole directory, losing any custom Cargo config. The recipe should create a temporary directory and only replace `.cargo` atomically after success.

## `tag` recipe in justfile uses `sed -i` with a fragile in-place substitution

`justfile:76` uses `find -type f -name Cargo.toml -exec sed -i '0,/^version/s/...'`. On macOS, `sed -i` requires an extension argument. Since this project targets Linux only, this is not a portability bug today, but it is worth noting as a platform assumption baked into tooling.

## Platform assumption: Linux-only with no conditional compilation

The entire codebase assumes Linux. `src/liquidctl.rs:109` spawns `liquidctl` directly by name with no fallback for non-Linux platforms. There is no `#[cfg(target_os = "linux")]` gate. Cross-compilation targets would produce a binary that panics or produces runtime errors at first poll. This is acceptable for the stated scope (Pop\!\_OS 24.04) but should be documented as an explicit constraint.

## `resources/app.metainfo.xml` missing udev dependency hint

The metainfo/AppStream file (referenced in `justfile:54`) does not appear to list a `requires` or `recommends` element for the `liquidctl` binary or a udev rules package. Package managers that parse AppStream data will not know to install `liquidctl` as a dependency.

## README mentions `sccache` in a link but installation instructions are absent

`README.md:43` has a `[sccache]` link target defined but no text referencing it in the document, and no instructions for enabling it. This is a dead link anchor.
