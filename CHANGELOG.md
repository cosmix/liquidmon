# Changelog

All notable user-facing changes to LiquidMon are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## 0.4.0

### Added

- Cooling control for the `hydro_platinum` device family (Corsair Hydro
  Pro XT, Platinum, and iCUE Elite RGB models): four presets (Silent,
  Balanced, Performance, Max), each pairing a fan curve with a pump mode,
  plus a manual mode with an all-fans duty slider (20–100%, step 5) and a
  three-way pump mode (Quiet / Balanced / Extreme).
- A curve preview in the popup showing the active preset's fan curve and a
  marker at the current coolant temperature.
- An "Apply now" button and a "Re-apply if the cooler loses it" switch, plus
  a status line reporting the last write and how many writes happened this
  session.

### Changed

- Settings applied through the new controls are written into the cooler
  itself, so they persist across a LiquidMon restart, a logout, and a
  reboot on systems whose PSU keeps standby power alive; they are lost only
  on a full power cut.
- The default control mode is Unmanaged, in which LiquidMon never writes to
  the cooler. Existing installs upgrade into Unmanaged and stay read-only
  until a mode is chosen. When a mode is active, LiquidMon writes
  automatically at most once per applet run, only after two consecutive
  status samples show the cooler has drifted from the saved setting — never
  on a timer, never unconditionally at startup.
