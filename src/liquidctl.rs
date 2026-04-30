// SPDX-License-Identifier: MPL-2.0

//! Integration with the `liquidctl` CLI for reading AIO cooler status.

use serde::Deserialize;
use std::fmt;
use std::io;
use std::time::Duration;
use tokio::process::Command;

/// Parsed status snapshot for a single AIO device.
#[derive(Debug, Clone)]
pub struct AioStatus {
    pub description: String,
    pub liquid_temp_c: f64,
    pub pump: Pump,
    pub fans: Vec<Fan>,
}

#[derive(Debug, Clone)]
pub struct Pump {
    pub speed_rpm: u32,
    pub duty_pct: u8,
}

#[derive(Debug, Clone)]
pub struct Fan {
    pub index: u8,
    pub speed_rpm: u32,
    pub duty_pct: u8,
}

#[derive(Debug)]
pub enum Error {
    Spawn(io::Error),
    NonZeroExit {
        status: Option<i32>,
        stderr: String,
    },
    Parse(serde_json::Error),
    NoDevice,
    MissingField(&'static str),
    Timeout,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "failed to spawn liquidctl: {e}"),
            Error::NonZeroExit { status, stderr } => match status {
                Some(code) => write!(
                    f,
                    "liquidctl exited with status {code}: {}",
                    stderr.trim()
                ),
                None => write!(
                    f,
                    "liquidctl terminated by signal: {}",
                    stderr.trim()
                ),
            },
            Error::Parse(e) => write!(f, "failed to parse liquidctl JSON output: {e}"),
            Error::NoDevice => write!(f, "no matching AIO device with usable status reported"),
            Error::Timeout => write!(f, "liquidctl call timed out"),
            Error::MissingField(field) => {
                write!(f, "device found but missing required status field: {field}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Spawn(e) => Some(e),
            Error::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Spawn(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Parse(e)
    }
}

/// Raw `liquidctl --json status` device entry.
#[derive(Debug, Deserialize)]
struct DeviceEntry {
    #[allow(dead_code)]
    bus: String,
    #[allow(dead_code)]
    address: String,
    description: String,
    status: Vec<StatusEntry>,
}

#[derive(Debug, Deserialize)]
struct StatusEntry {
    key: String,
    value: serde_json::Number,
    #[allow(dead_code)]
    unit: String,
}

/// Runs `liquidctl --match <match_filter> --json status`, parses the first
/// device with a non-empty `status` array, and returns its parsed AioStatus.
pub async fn fetch_status(match_filter: &str) -> Result<AioStatus, Error> {
    let mut cmd = Command::new("liquidctl");
    cmd.args(["--match", match_filter, "--json", "status"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), cmd.output())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::Spawn)?;

    if !output.status.success() {
        return Err(Error::NonZeroExit {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let raw = std::str::from_utf8(&output.stdout).map_err(|e| {
        Error::Spawn(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("liquidctl produced non-UTF-8 stdout: {e}"),
        ))
    })?;
    parse_status_response(raw)
}

/// Parses raw `liquidctl --json status` output and returns the first device
/// with a non-empty status array decoded into an [`AioStatus`].
fn parse_status_response(raw: &str) -> Result<AioStatus, Error> {
    let devices: Vec<DeviceEntry> = serde_json::from_str(raw)?;

    let device = devices
        .into_iter()
        .find(|d| !d.status.is_empty())
        .ok_or(Error::NoDevice)?;

    // Bounded casts: percentages clamp to 0–100; RPM/counts clamp to u32 domain.
    let to_u8_pct = |v: f64| v.clamp(0.0, 100.0) as u8;
    let to_u32 = |v: f64| v.clamp(0.0, u32::MAX as f64) as u32;

    let mut liquid_temp_c: Option<f64> = None;
    let mut pump_speed: Option<u32> = None;
    let mut pump_duty: Option<u8> = None;
    // Per-fan accumulator, keyed by index.
    let mut fans: std::collections::BTreeMap<u8, (Option<u32>, Option<u8>)> =
        std::collections::BTreeMap::new();

    for entry in &device.status {
        let Some(value_f64) = entry.value.as_f64() else {
            continue;
        };

        match entry.key.as_str() {
            "Liquid temperature" => liquid_temp_c = Some(value_f64),
            "Pump speed" => pump_speed = Some(to_u32(value_f64)),
            "Pump duty" => pump_duty = Some(to_u8_pct(value_f64)),
            other => {
                if let Some(rest) = other.strip_prefix("Fan ") {
                    if let Some((num, suffix)) = split_fan_key(rest) {
                        let slot = fans.entry(num).or_insert((None, None));
                        match suffix {
                            "speed" => slot.0 = Some(to_u32(value_f64)),
                            "duty" => slot.1 = Some(to_u8_pct(value_f64)),
                            _ => {}
                        }
                    }
                }
                // Unrecognized keys are silently ignored.
            }
        }
    }

    let liquid_temp_c = liquid_temp_c.ok_or(Error::MissingField("liquid temperature"))?;
    let pump_speed_rpm = pump_speed.ok_or(Error::MissingField("pump speed"))?;
    let pump_duty_pct = pump_duty.ok_or(Error::MissingField("pump duty"))?;

    let mut fan_list: Vec<Fan> = fans
        .into_iter()
        .filter_map(|(index, (speed, duty))| match (speed, duty) {
            (Some(speed_rpm), Some(duty_pct)) => Some(Fan {
                index,
                speed_rpm,
                duty_pct,
            }),
            _ => None,
        })
        .collect();
    fan_list.sort_by_key(|f| f.index);

    Ok(AioStatus {
        description: device.description,
        liquid_temp_c,
        pump: Pump {
            speed_rpm: pump_speed_rpm,
            duty_pct: pump_duty_pct,
        },
        fans: fan_list,
    })
}

/// Given the portion of a status key after `"Fan "`, return the fan index
/// (a positive integer) and the trailing suffix (e.g. `"speed"` or `"duty"`),
/// or `None` if the key does not match the `Fan N <suffix>` pattern.
fn split_fan_key(rest: &str) -> Option<(u8, &str)> {
    let (num_str, suffix) = rest.split_once(' ')?;
    let num: u8 = num_str.parse().ok()?;
    if num == 0 {
        return None;
    }
    Some((num, suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[{"bus": "hid", "address": "/dev/hidraw0", "description": "Gigabyte RGB Fusion 2.0 8297 Controller", "status": []}, {"bus": "hid", "address": "/dev/hidraw1", "description": "Corsair Hydro H150i Pro XT", "status": [{"key": "Liquid temperature", "value": 30.109803921568627, "unit": "°C"}, {"key": "Fan 1 speed", "value": 1000, "unit": "rpm"}, {"key": "Fan 1 duty", "value": 40, "unit": "%"}, {"key": "Fan 2 speed", "value": 971, "unit": "rpm"}, {"key": "Fan 2 duty", "value": 40, "unit": "%"}, {"key": "Fan 3 speed", "value": 1034, "unit": "rpm"}, {"key": "Fan 3 duty", "value": 40, "unit": "%"}, {"key": "Pump speed", "value": 2334, "unit": "rpm"}, {"key": "Pump duty", "value": 75, "unit": "%"}]}]"#;

    #[test]
    fn parses_h150i_pro_xt_fixture() {
        let status = parse_status_response(FIXTURE).expect("fixture should parse");

        assert_eq!(status.description, "Corsair Hydro H150i Pro XT");
        assert!(
            (status.liquid_temp_c - 30.1098).abs() < 0.01,
            "liquid_temp_c was {}",
            status.liquid_temp_c
        );

        assert_eq!(status.pump.speed_rpm, 2334);
        assert_eq!(status.pump.duty_pct, 75);

        assert_eq!(status.fans.len(), 3);

        assert_eq!(status.fans[0].index, 1);
        assert_eq!(status.fans[0].speed_rpm, 1000);
        assert_eq!(status.fans[0].duty_pct, 40);

        assert_eq!(status.fans[1].index, 2);
        assert_eq!(status.fans[1].speed_rpm, 971);
        assert_eq!(status.fans[1].duty_pct, 40);

        assert_eq!(status.fans[2].index, 3);
        assert_eq!(status.fans[2].speed_rpm, 1034);
        assert_eq!(status.fans[2].duty_pct, 40);
    }

    #[test]
    fn empty_array_yields_no_device() {
        match parse_status_response("[]") {
            Err(Error::NoDevice) => {}
            other => panic!("expected Error::NoDevice, got {other:?}"),
        }
    }

    #[test]
    fn all_devices_empty_status_yields_no_device() {
        let raw = r#"[
            {"bus":"hid","address":"/dev/hidraw0","description":"A","status":[]},
            {"bus":"hid","address":"/dev/hidraw1","description":"B","status":[]}
        ]"#;
        match parse_status_response(raw) {
            Err(Error::NoDevice) => {}
            other => panic!("expected Error::NoDevice, got {other:?}"),
        }
    }

    /// Device present with a non-empty status array, but the "Liquid temperature"
    /// entry is absent — should surface as MissingField, not NoDevice.
    #[test]
    fn device_missing_liquid_temp_yields_missing_field() {
        // Fixture is the H150i but with the Liquid temperature entry removed.
        let raw = r#"[{"bus": "hid", "address": "/dev/hidraw1", "description": "Corsair Hydro H150i Pro XT", "status": [{"key": "Fan 1 speed", "value": 1000, "unit": "rpm"}, {"key": "Fan 1 duty", "value": 40, "unit": "%"}, {"key": "Pump speed", "value": 2334, "unit": "rpm"}, {"key": "Pump duty", "value": 75, "unit": "%"}]}]"#;
        match parse_status_response(raw) {
            Err(Error::MissingField("liquid temperature")) => {}
            other => panic!(
                "expected Error::MissingField(\"liquid temperature\"), got {other:?}"
            ),
        }
    }
}
