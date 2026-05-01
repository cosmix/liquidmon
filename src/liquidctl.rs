// SPDX-License-Identifier: MPL-2.0

//! Integration with the `liquidctl` CLI for reading AIO cooler status.

use serde::Deserialize;
use std::fmt;
use std::io;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Serializes all `liquidctl` subprocess calls to prevent concurrent HID
/// device access conflicts (the driver opens an exclusive `O_RDWR` claim).
static LIQUIDCTL_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

/// A device returned by `liquidctl list --json`.
#[derive(Debug, Clone)]
pub struct DetectedDevice {
    pub description: String,
    // bus/address are parsed and exposed for a follow-up plan that will
    // disambiguate identical AIOs via `--bus X --address Y`. v1 selects
    // by description only; the fields ride through unread for now.
    #[allow(dead_code)]
    pub bus: String,
    #[allow(dead_code)]
    pub address: String,
}

#[derive(Debug)]
pub enum Error {
    Spawn(io::Error),
    NonZeroExit { status: Option<i32>, stderr: String },
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
                Some(code) => write!(f, "liquidctl exited with status {code}: {}", stderr.trim()),
                None => write!(f, "liquidctl terminated by signal: {}", stderr.trim()),
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
    let _guard = LIQUIDCTL_LOCK.lock().await;
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
                if let Some(rest) = other.strip_prefix("Fan ")
                    && let Some((num, suffix)) = split_fan_key(rest)
                {
                    let slot = fans.entry(num).or_insert((None, None));
                    match suffix {
                        "speed" => slot.0 = Some(to_u32(value_f64)),
                        "duty" => slot.1 = Some(to_u8_pct(value_f64)),
                        _ => {}
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

/// Runs `liquidctl list --json` (no `--match`), enumerates every device
/// liquidctl can see, and returns the parsed list.
///
/// A 1 s timeout is used — `list` is purely an HID enumeration with no
/// on-device transaction, so 3 s is unnecessarily generous.
pub async fn list_devices() -> Result<Vec<DetectedDevice>, Error> {
    let _guard = LIQUIDCTL_LOCK.lock().await;
    let mut cmd = Command::new("liquidctl");
    cmd.args(["list", "--json"]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(1), cmd.output())
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
    parse_devices_response(raw)
}

/// Parses raw `liquidctl list --json` output into a list of [`DetectedDevice`].
///
/// Returns `Ok(vec![])` when liquidctl reports no devices (an empty JSON array
/// is valid — this is distinct from [`Error::NoDevice`], which only
/// `fetch_status` produces when no device has a usable status array).
fn parse_devices_response(raw: &str) -> Result<Vec<DetectedDevice>, Error> {
    let entries: Vec<ListDeviceEntry> = serde_json::from_str(raw)?;
    Ok(entries
        .into_iter()
        .map(|e| DetectedDevice {
            description: e.description,
            bus: e.bus,
            address: e.address,
        })
        .collect())
}

/// Private deserialization helper for `liquidctl list --json` entries.
/// Fields beyond `description`, `bus`, and `address` are intentionally
/// omitted — serde ignores unknown keys by default.
#[derive(Debug, Deserialize)]
struct ListDeviceEntry {
    description: String,
    /// Tolerant: liquidctl normally emits a string (`"hid"`, `"usb"`), but
    /// defensive deserialization via `serde_json::Value` protects against
    /// future driver changes that emit null or a numeric value.
    #[serde(deserialize_with = "deserialize_string_lossy")]
    bus: String,
    /// Same defensive treatment — USB addresses can in principle be numeric
    /// tuples in some liquidctl backends.
    #[serde(deserialize_with = "deserialize_string_lossy")]
    address: String,
}

fn deserialize_string_lossy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
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
            other => panic!("expected Error::MissingField(\"liquid temperature\"), got {other:?}"),
        }
    }

    #[test]
    fn device_missing_pump_speed_yields_missing_field() {
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump duty","value":75,"unit":"%"}
        ]}]"#;
        match parse_status_response(raw) {
            Err(Error::MissingField("pump speed")) => {}
            other => panic!("expected MissingField(\"pump speed\"), got {other:?}"),
        }
    }

    #[test]
    fn device_missing_pump_duty_yields_missing_field() {
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"}
        ]}]"#;
        match parse_status_response(raw) {
            Err(Error::MissingField("pump duty")) => {}
            other => panic!("expected MissingField(\"pump duty\"), got {other:?}"),
        }
    }

    #[test]
    fn fan_with_only_speed_is_dropped() {
        // Fan 1 has speed but no duty -> should be filtered out; Fan 2 is complete.
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":75,"unit":"%"},
            {"key":"Fan 1 speed","value":1000,"unit":"rpm"},
            {"key":"Fan 2 speed","value":1100,"unit":"rpm"},
            {"key":"Fan 2 duty","value":50,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].index, 2);
        assert_eq!(status.fans[0].speed_rpm, 1100);
        assert_eq!(status.fans[0].duty_pct, 50);
    }

    #[test]
    fn fan_with_only_duty_is_dropped() {
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":75,"unit":"%"},
            {"key":"Fan 1 duty","value":40,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert!(status.fans.is_empty());
    }

    #[test]
    fn fan_index_zero_is_ignored() {
        // Fan 0 should be silently dropped per split_fan_key; Fan 1 retained.
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":75,"unit":"%"},
            {"key":"Fan 0 speed","value":900,"unit":"rpm"},
            {"key":"Fan 0 duty","value":30,"unit":"%"},
            {"key":"Fan 1 speed","value":1000,"unit":"rpm"},
            {"key":"Fan 1 duty","value":40,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].index, 1);
    }

    #[test]
    fn fans_emerge_sorted_by_index() {
        // Insert keys in shuffled order; result should still be ordered 1,2,3.
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":75,"unit":"%"},
            {"key":"Fan 3 speed","value":1300,"unit":"rpm"},
            {"key":"Fan 1 duty","value":40,"unit":"%"},
            {"key":"Fan 2 speed","value":1200,"unit":"rpm"},
            {"key":"Fan 3 duty","value":60,"unit":"%"},
            {"key":"Fan 1 speed","value":1100,"unit":"rpm"},
            {"key":"Fan 2 duty","value":50,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        let indices: Vec<u8> = status.fans.iter().map(|f| f.index).collect();
        assert_eq!(indices, vec![1, 2, 3]);
    }

    #[test]
    fn out_of_range_pump_duty_is_clamped() {
        // Duty 250 should clamp to 100; Pump speed 5_000_000_000 (> u32 cast safe band) is fine for u32 but ridiculous; choose a value within u32.
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":250,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.pump.duty_pct, 100);
    }

    #[test]
    fn negative_values_clamp_to_zero() {
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":-50,"unit":"rpm"},
            {"key":"Pump duty","value":-10,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.pump.speed_rpm, 0);
        assert_eq!(status.pump.duty_pct, 0);
    }

    #[test]
    fn first_device_with_status_is_selected() {
        // Three devices: first empty, second populated, third populated -> second wins.
        let raw = r#"[
            {"bus":"hid","address":"/dev/hidraw0","description":"Empty","status":[]},
            {"bus":"hid","address":"/dev/hidraw1","description":"Picked","status":[
                {"key":"Liquid temperature","value":31.0,"unit":"°C"},
                {"key":"Pump speed","value":2000,"unit":"rpm"},
                {"key":"Pump duty","value":75,"unit":"%"}
            ]},
            {"bus":"hid","address":"/dev/hidraw2","description":"Skipped","status":[
                {"key":"Liquid temperature","value":99.0,"unit":"°C"},
                {"key":"Pump speed","value":1,"unit":"rpm"},
                {"key":"Pump duty","value":1,"unit":"%"}
            ]}
        ]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.description, "Picked");
        assert!((status.liquid_temp_c - 31.0).abs() < 0.001);
    }

    #[test]
    fn unknown_keys_are_silently_ignored() {
        // "Firmware version" is unknown — must not affect parsing.
        let raw = r#"[{"bus":"hid","address":"/dev/hidraw1","description":"X","status":[
            {"key":"Firmware version","value":42,"unit":""},
            {"key":"Liquid temperature","value":30.0,"unit":"°C"},
            {"key":"Pump speed","value":2000,"unit":"rpm"},
            {"key":"Pump duty","value":75,"unit":"%"}
        ]}]"#;
        let status = parse_status_response(raw).expect("should parse");
        assert_eq!(status.pump.speed_rpm, 2000);
    }

    #[test]
    fn malformed_json_yields_parse_error() {
        match parse_status_response("not json at all") {
            Err(Error::Parse(_)) => {}
            other => panic!("expected Error::Parse, got {other:?}"),
        }
    }

    #[test]
    fn split_fan_key_extracts_index_and_suffix() {
        assert_eq!(split_fan_key("1 speed"), Some((1, "speed")));
        assert_eq!(split_fan_key("12 duty"), Some((12, "duty")));
    }

    #[test]
    fn split_fan_key_rejects_zero_and_malformed() {
        assert_eq!(split_fan_key("0 speed"), None);
        assert_eq!(split_fan_key("abc speed"), None);
        assert_eq!(split_fan_key("1"), None);
        assert_eq!(split_fan_key(""), None);
    }

    #[test]
    fn display_includes_field_name_for_missing_field() {
        let s = format!("{}", Error::MissingField("liquid temperature"));
        assert!(s.contains("liquid temperature"), "got: {s}");
    }

    #[test]
    fn display_for_no_device_and_timeout() {
        assert!(!format!("{}", Error::NoDevice).is_empty());
        assert!(format!("{}", Error::Timeout).contains("timed out"));
    }

    #[test]
    fn error_source_chains_for_inner_io_and_parse() {
        use std::error::Error as _;
        let io_err = io::Error::other("boom");
        let e = Error::Spawn(io_err);
        assert!(e.source().is_some());

        let parse_err = serde_json::from_str::<Vec<DeviceEntry>>("not json")
            .err()
            .unwrap();
        let e = Error::Parse(parse_err);
        assert!(e.source().is_some());

        // NoDevice / MissingField / Timeout / NonZeroExit have no inner source.
        assert!(Error::NoDevice.source().is_none());
        assert!(Error::MissingField("x").source().is_none());
        assert!(Error::Timeout.source().is_none());
    }

    // ── list_devices / parse_devices_response tests ──────────────────────────

    const LIST_FIXTURE: &str = r#"[{"bus":"hid","address":"/dev/hidraw0","description":"Gigabyte RGB Fusion 2.0 8297 Controller","vendor_id":6357,"product_id":33122,"release_number":256,"serial_number":"5C7E0EFAA1E5","port":"3","driver":"RgbFusion2","experimental":false},{"bus":"hid","address":"/dev/hidraw1","description":"Corsair Hydro H150i Pro XT","vendor_id":6940,"product_id":3107,"release_number":256,"serial_number":"1234ABCD","port":"4","driver":"HydroPlatinum","experimental":false}]"#;

    #[test]
    fn parses_multiple_devices_from_list_fixture() {
        let devices = parse_devices_response(LIST_FIXTURE).expect("fixture should parse");
        assert_eq!(devices.len(), 2);

        assert_eq!(
            devices[0].description,
            "Gigabyte RGB Fusion 2.0 8297 Controller"
        );
        assert_eq!(devices[0].bus, "hid");
        assert_eq!(devices[0].address, "/dev/hidraw0");

        assert_eq!(devices[1].description, "Corsair Hydro H150i Pro XT");
        assert_eq!(devices[1].bus, "hid");
        assert_eq!(devices[1].address, "/dev/hidraw1");
    }

    #[test]
    fn empty_list_yields_empty_vec() {
        // An empty JSON array is valid for `liquidctl list` — no device is
        // connected. This is distinct from Error::NoDevice, which only
        // fetch_status produces when devices are present but none has a
        // usable status array.
        let result = parse_devices_response("[]").expect("empty array should not fail");
        assert!(
            result.is_empty(),
            "expected empty vec (not Error::NoDevice) for empty list output"
        );
    }

    #[test]
    fn malformed_list_yields_parse_error() {
        match parse_devices_response("garbage") {
            Err(Error::Parse(_)) => {}
            other => panic!("expected Error::Parse, got {other:?}"),
        }
    }

    #[test]
    fn list_device_entry_ignores_unknown_fields() {
        // Minimal fixture: only the three fields we care about plus one unknown.
        let raw = r#"[{"description":"Test Device","bus":"hid","address":"/dev/hidraw0","experimental":true}]"#;
        let devices = parse_devices_response(raw)
            .expect("entry with unknown fields should deserialize successfully");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].description, "Test Device");
        assert_eq!(devices[0].bus, "hid");
        assert_eq!(devices[0].address, "/dev/hidraw0");
    }
}
