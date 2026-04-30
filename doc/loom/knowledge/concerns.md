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

## No app-level tests for `app.rs::update`

`src/main.rs`, `src/app.rs`, and `src/config.rs` have no `#[cfg(test)]` modules. The UI logic (`view`, `view_window`, `update`) is untested. At minimum, the `update` method's state transitions (`StatusTick(Ok)`, `StatusTick(Err)`, `PopupClosed`) should have unit tests.

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
