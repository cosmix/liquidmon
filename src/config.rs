// SPDX-License-Identifier: MPL-2.0

use crate::control::{ControlMode, Preset, PumpMode};
use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 4]
pub struct Config {
    pub sample_interval_ms: u64,
    /// User-selected liquidctl device description (verbatim, used as
    /// `--match` substring). `None` means auto-detect at runtime.
    pub device_match: Option<String>,
    /// Active control mode. `Unmanaged` means LiquidMon never writes.
    pub control_mode: ControlMode,
    /// Last preset chosen, retained across a Manual detour.
    pub control_preset: Preset,
    /// Last manual fan duty, clamped to
    /// [`control::MIN_FAN_DUTY`](crate::control::MIN_FAN_DUTY) ..=
    /// [`control::MAX_FAN_DUTY`](crate::control::MAX_FAN_DUTY).
    pub manual_fan_duty: u8,
    /// Last manual pump mode. This family exposes modes, not a pump duty.
    pub manual_pump_mode: PumpMode,
    /// Allow one automatic write per applet run when the cooler is observed
    /// to have lost the setting. `false` means writes are user-initiated only.
    pub auto_reapply: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1500,
            device_match: None,
            // Unmanaged by default: an upgrade from an older config version
            // must never start writing to hardware on its own. Every field
            // below is inert until the user picks a mode.
            control_mode: ControlMode::Unmanaged,
            control_preset: Preset::Balanced,
            manual_fan_duty: 50,
            manual_pump_mode: PumpMode::Balanced,
            auto_reapply: true,
        }
    }
}
