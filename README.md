# LiquidMon

LiquidMon is a COSMIC panel applet that monitors Corsair Hydro AIO coolers in
real time via the [`liquidctl`][liquidctl] CLI. The panel button shows the
current liquid temperature, a rolling 90-second sparkline of recent samples,
average fan duty %, and pump duty %. Clicking it opens a popup with the full
device description, liquid temperature, pump speed and duty, and per-fan
speed and duty for every fan the AIO reports.

The applet polls `liquidctl --json status` every 1.5 seconds with a 3-second
per-call timeout. If a poll fails, the last successful reading is kept on
display and the popup shows the underlying error — so a frozen temperature
reading combined with an error in the popup means liquidctl has stopped
responding.

App ID: `com.github.cosmix.LiquidMon`

## Supported Devices

Any Corsair AIO whose `liquidctl status` description contains the word `Hydro`
— for example: Hydro H100i, H115i, H150i Pro XT, H170i.

> **Note:** the device match filter is currently hardcoded to `"Hydro"`.
> Configurable matching is a future feature.

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

The most recent `liquidctl` call failed. Reproduce the underlying error
from a terminal:

```sh
liquidctl --match Hydro --json status
```

Common causes:

- udev rules missing — see [udev rules](#udev-rules)
- AIO unplugged or in a bad state
- `liquidctl` not installed or not on `PATH`

**Panel shows `…`**

No reading has arrived yet. Polling runs every 1.5 seconds with a 3-second
per-call timeout, so a steady `…` for more than 5 seconds means liquidctl
is hanging or the subscription failed to start. Check the COSMIC panel log:

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
just run                  # build release and run with RUST_BACKTRACE=full
```

Vendored offline builds:

```sh
just vendor && just build-vendored
```

## License

MPL-2.0 — see `LICENSE`.

[liquidctl]: https://github.com/liquidctl/liquidctl
[just]: https://github.com/casey/just
[rustup]: https://rustup.rs
[releases]: https://github.com/cosmix/liquidmon/releases
