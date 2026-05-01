# LiquidMon

LiquidMon is a COSMIC panel applet that monitors AIO liquid coolers in
real time via the [`liquidctl`][liquidctl] CLI. The panel button shows the
current liquid temperature, a gradient-filled sparkline of recent samples,
average fan duty %, and pump duty %. Clicking it opens a popup with the
device description, three large 15-minute sparklines (coolant temperature,
pump duty, fan-average duty), and a slider that sets the sample interval
(1.0 s – 10.0 s, persisted via cosmic-config).

The applet polls `liquidctl --json status` at the configured interval (default
1.5 s) with a 3-second per-call timeout. If a poll fails, the last successful
reading is kept on display and the popup shows the underlying error — so a
frozen temperature reading combined with an error in the popup means liquidctl
has stopped responding.

App ID: `com.github.cosmix.LiquidMon`

## Supported Devices

LiquidMon ships v1 with verified support for two AIO families:

- **Corsair Hydro Pro / Pro XT / Platinum** (e.g. H100i, H115i, H150i Pro XT, H170i)
- **Corsair iCUE Elite Capellix / RGB** (e.g. H100i Elite Capellix, H150i Elite RGB)

Connected liquidctl devices are enumerated on launch and when the popup is
opened. The first compatible AIO is auto-selected; a dropdown in the popup
lets you override the choice when more than one is connected. The selected
device is persisted via cosmic-config and survives applet restart.

Broader liquidctl device support (NZXT Kraken, EVGA CLC, Aquacomputer D5
Next, MSI Coreliquid, ASUS Ryujin, Lian Li Galahad II LCD) is planned —
those families ship under a separate parser change.

### Limitations

- **One device per cooler family.** When two identical AIOs are connected,
  liquidctl's substring match selects the first; v1 cannot disambiguate
  truly-identical devices.
- **Hot-plug detected on popup open.** Plugging in a new cooler does not
  auto-update the panel; open the popup once to trigger re-enumeration.

## Install

### From a release (recommended)

Download the latest `.deb` from the [Releases][releases] page and install it
with `apt`, which pulls in `liquidctl` (and its HID udev rules) as a
dependency:

```sh
sudo apt install ./liquidmon_*.deb
```

If `/dev/hidraw*` nodes for the AIO already existed, replug the AIO's
internal USB header (or reboot) so they pick up the new permissions.

### From source

Install the build dependencies (matches CI):

```sh
sudo apt install \
    pkg-config \
    libxkbcommon-dev \
    libwayland-dev \
    libfontconfig1-dev \
    libfreetype6-dev \
    liquidctl
```

A stable Rust toolchain is also required ([`rustup`][rustup]).

Build and install:

```sh
cargo build --release
sudo install -Dm0755 target/release/liquidmon /usr/bin/liquidmon
sudo install -Dm0644 resources/app.desktop /usr/share/applications/com.github.cosmix.LiquidMon.desktop
sudo install -Dm0644 resources/app.metainfo.xml /usr/share/appdata/com.github.cosmix.LiquidMon.metainfo.xml
sudo install -Dm0644 resources/icon.svg /usr/share/icons/hicolor/scalable/apps/com.github.cosmix.LiquidMon.svg
```

If you have [`just`][just], `sudo just install` runs the four `install`
commands above.

### Uninstall

```sh
sudo apt remove liquidmon          # if installed via .deb
sudo just uninstall                # if installed from source
```

## udev rules

On Debian/Ubuntu the `liquidctl` apt package ships HID udev rules to
`/lib/udev/rules.d/71-liquidctl.rules`, so installs that go through `apt`
— including the `.deb` install path above — already have them.

If liquidctl was installed another way and the applet shows `!`, install
the upstream rules manually:

```sh
sudo ./scripts/install-liquidctl-udev.sh
```

Then replug the AIO's internal USB header (or reboot) so existing
`/dev/hidraw*` nodes pick up the new permissions.

## Troubleshooting

**Panel shows `!`**

The most recent `liquidctl` call failed. Enumerate connected devices and
reproduce the underlying error from a terminal:

```sh
liquidctl list --json
liquidctl --match "<full description>" --json status
```

Common causes:

- udev rules missing — see [udev rules](#udev-rules)
- AIO unplugged or in a bad state
- `liquidctl` not installed or not on `PATH`

**Panel shows `…`**

No reading has arrived yet. Polling runs at the configured sample interval
(1.5 s by default) with a 3-second per-call timeout, so a steady `…` for
more than the interval plus 3 s means liquidctl is hanging or the
subscription failed to start. Check the COSMIC panel log:

```sh
journalctl --user -u cosmic-panel
```

**Panel reading appears frozen**

A stale reading is preserved when polls start failing. Open the popup — the
underlying error is shown at the bottom.

## Development

```sh
cargo test                # run unit tests
just check                # cargo clippy --all-features -- -W clippy::pedantic
just ci-local             # fmt --check + clippy -D warnings + test + release build
just run                  # build release and run with RUST_BACKTRACE=full
```

Vendored offline builds:

```sh
just vendor && just build-vendored
```

### Git hooks

The repo ships fast-feedback hooks in `.githooks/` that mirror CI so a
push doesn't fail after a tag is cut. Wire them into your local clone
once with:

```sh
just hooks
# or, without `just`:
./.githooks/install.sh
```

That points `core.hooksPath` at `.githooks/`, giving you:

- `pre-commit` — `cargo fmt --check` and `cargo clippy -D warnings` on
  any commit that touches `*.rs`, `Cargo.toml`, or `Cargo.lock`. Skipped
  on rebases and merges.
- `pre-push` — `cargo test --all-features` plus `cargo audit` (if
  installed) and a tag-vs-Cargo.toml version match check on tag pushes.

Optional dependency-CVE scanning needs `cargo install cargo-audit --locked`
once. Push-time bypasses if you really need them: `git push --no-verify`
or `LIQUIDMON_SKIP_AUDIT=1 git push ...` (skips just the audit step).

### Running as a standalone window

When the binary is launched outside the COSMIC panel, libcosmic falls back
to rendering the applet as a small standalone Wayland window — the panel
button becomes a clickable window, and clicking it opens the popup as an
`xdg_popup` anchored to that window. This is the fastest way to iterate
on UI changes without reinstalling into `/usr/bin` or restarting
`cosmic-panel`.

```sh
just run
# or, with a debug build:
cargo run
```

A few `COSMIC_PANEL_*` environment variables that the panel normally sets
can be set manually to influence layout — useful for testing different
panel configurations:

```sh
# Render the applet button at a larger size (XS|S|M|L|XL or Custom(N))
COSMIC_PANEL_SIZE=XL just run

# Pretend we're docked to the bottom edge (Top|Bottom|Left|Right)
COSMIC_PANEL_ANCHOR=Bottom just run
```

Requirements and caveats:

- A Wayland session is required (the COSMIC desktop or another Wayland
  compositor that supports `xdg_popup`). On X11/XWayland the popup
  positioning APIs are unavailable.
- `liquidctl` must be on `PATH` and the udev rules must be installed —
  the standalone window calls the same subprocess as the installed
  applet, so a missing rule produces the same `!` error state.
- `RUST_LOG=liquidmon=debug,cosmic=warn cargo run` raises the log level
  for the applet without flooding the terminal with libcosmic chatter.
- The standalone window has no decorations; close it with the
  compositor's window close shortcut (`Super+Q` on COSMIC) or `Ctrl+C`
  in the terminal.

## License

MPL-2.0 — see `LICENSE`.

[liquidctl]: https://github.com/liquidctl/liquidctl
[just]: https://github.com/casey/just
[rustup]: https://rustup.rs
[releases]: https://github.com/cosmix/liquidmon/releases
