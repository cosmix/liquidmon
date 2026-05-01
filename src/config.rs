// SPDX-License-Identifier: MPL-2.0

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 2]
pub struct Config {
    pub sample_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_interval_ms: 1500,
        }
    }
}
