# LiquidMon

LiquidMon is a COSMIC panel applet that monitors Corsair Hydro AIO coolers in
real time via the [`liquidctl`][liquidctl] CLI. The panel button displays the
current liquid temperature, a rolling sparkline of recent temperature samples,
average fan duty %, and pump duty %. Clicking the button opens a popup with the
full device description, liquid temperature, pump speed and duty, and per-fan
speed and duty for every fan the AIO reports.

<!-- TODO: replace with an actual screenshot once the applet is running -->
<!-- ![LiquidMon panel applet](docs/screenshot.png) -->

## Prerequisites

- **COSMIC desktop** — stable release (Pop!\_OS 24.04 or any compatible distribution)
- **`liquidctl`** — install via `pip install liquidctl` or your distro's package
  manager (e.g. `sudo apt install liquidctl` on Ubuntu/Pop!\_OS)
- **HID udev rules** — without these `liquidctl` requires `sudo` and the applet
  will only ever show the `!` error state

  Run the bundled script once to install the upstream udev rules and reload them:

  ```sh
  sudo ./scripts/install-liquidctl-udev.sh
  ```

  If `liquidctl status` still requires `sudo` after running the script,
  unplug and replug the AIO's internal USB header (or reboot) to rebind
  the `/dev/hidraw*` node under the new permissions.

## Install

```sh
just build-release
sudo just install
```

This installs the `liquidmon` binary to `/usr/bin/`, the `.desktop` launcher to
`/usr/share/applications/`, the metainfo file to `/usr/share/appdata/`, and the
icon to `/usr/share/icons/`.

## Supported Devices

Any Corsair AIO whose `liquidctl status` description contains the word `Hydro`
— for example: Hydro H100i, H115i, H150i Pro XT, H170i.

> **Note:** the device match filter is currently hardcoded to `"Hydro"`.
> Support for configuring the filter is a future feature.

## Troubleshooting

**Panel shows `!` (error state)**

Run the following from a terminal to see the underlying error:

```sh
liquidctl --match Hydro --json status
```

If the command requires `sudo`, the udev rules are missing or stale — re-run
`sudo ./scripts/install-liquidctl-udev.sh` and replug the AIO's USB header.

**Panel shows `…` (waiting) for more than 5 seconds**

Check the COSMIC panel log for diagnostic messages:

```sh
journalctl --user -u cosmic-panel
```

## Development

```sh
just run          # build release and run with RUST_BACKTRACE=full
just check        # cargo clippy --all-features -W clippy::pedantic
cargo test        # run unit tests
```

Vendored offline builds:

```sh
just vendor && just build-vendored
```

## License

MPL-2.0 — see `LICENSE` if/when one is added to the repository.

[liquidctl]: https://github.com/liquidctl/liquidctl
[just]: https://github.com/casey/just
