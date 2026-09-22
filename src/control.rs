// SPDX-License-Identifier: MPL-2.0

//! Cooling-control model: presets, manual settings, per-family capability
//! lookup, and the pure `liquidctl` argv builders that apply them.

use crate::liquidctl::{AioStatus, Fan};
use serde::{Deserialize, Serialize};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Lowest fan duty the UI will request. The driver accepts 0, but most
/// 120/140 mm fans stall below ~20% and a stalled fan reads as a fault.
pub const MIN_FAN_DUTY: u8 = 20;
pub const MAX_FAN_DUTY: u8 = 100;

/// liquidctl's `normalize_profile` appends this point to every curve it
/// sends, so a preset curve of at most 6 points always fits the 7-point
/// device limit. `apply_steps` never sends it explicitly.
pub const CURVE_FAILSAFE: (u8, u8) = (60, 100);

/// Control capability of the driver behind a device description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// `hydro_platinum`: all-fan duty + curve, pump mode only.
    FanDutyAndPumpMode,
    /// No verified write path — controls render read-only.
    None,
}

/// Classify a liquidctl device description, case-insensitively. Three
/// ordered substring rules — "platinum", "pro xt", "elite rgb" — which
/// together cover every `hydro_platinum` description in liquidctl 1.15.0
/// and exclude `asetek_pro`'s plain "Hydro H150i Pro".
pub fn capability(description: &str) -> Capability {
    let d = description.to_ascii_lowercase();
    if d.contains("platinum") || d.contains("pro xt") || d.contains("elite rgb") {
        Capability::FanDutyAndPumpMode
    } else {
        Capability::None
    }
}

/// What LiquidMon is currently driving the cooler with.
///
/// Its `Default` is derived rather than hand-written: unlike `Config`'s
/// (whose body sets several distinct literal field values, a shape the
/// derive macro cannot express), this is exactly one unit variant, which is
/// what `#[derive(Default)]` + `#[default]` produces natively.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ControlMode {
    /// LiquidMon never writes to the device (default, and what every
    /// existing install upgrades into).
    #[default]
    Unmanaged,
    /// The payload carries the preset itself, so `resolve()` never has to
    /// consult `Settings.preset` (see that field's doc).
    Preset(Preset),
    Manual,
}

/// A named device-side fan curve plus pump mode.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Preset {
    Silent,
    #[default]
    Balanced,
    Performance,
    Max,
}

/// Curve points are `(liquid °C, fan duty %)`, each already monotonic and
/// at most 6 points so `_prepare_profile` can append the failsafe without
/// overflowing the device's 7-point limit.
const SILENT_CURVE: [(u8, u8); 5] = [(25, 20), (30, 25), (35, 35), (40, 50), (45, 75)];
const BALANCED_CURVE: [(u8, u8); 5] = [(25, 25), (30, 35), (35, 50), (40, 70), (45, 90)];
const PERFORMANCE_CURVE: [(u8, u8); 5] = [(25, 40), (30, 55), (35, 70), (40, 85), (45, 100)];

impl Preset {
    /// The preset's curve, or `None` for `Max`, which drives the fan at a
    /// fixed duty instead of a curve.
    pub fn curve(self) -> Option<&'static [(u8, u8)]> {
        match self {
            Preset::Silent => Some(&SILENT_CURVE),
            Preset::Balanced => Some(&BALANCED_CURVE),
            Preset::Performance => Some(&PERFORMANCE_CURVE),
            Preset::Max => None,
        }
    }
}

/// Pump mode bundled with each preset, per the preset table.
fn preset_pump_mode(preset: Preset) -> PumpMode {
    match preset {
        Preset::Silent => PumpMode::Quiet,
        Preset::Balanced => PumpMode::Balanced,
        Preset::Performance | Preset::Max => PumpMode::Extreme,
    }
}

/// Pump speed setting. `hydro_platinum` exposes no pump duty write, so these
/// three firmware modes are the whole pump control surface on this family.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PumpMode {
    Quiet,
    #[default]
    Balanced,
    Extreme,
}

impl PumpMode {
    /// The `initialize --pump-mode <arg>` value for this mode.
    pub fn as_arg(self) -> &'static str {
        match self {
            PumpMode::Quiet => "quiet",
            PumpMode::Balanced => "balanced",
            PumpMode::Extreme => "extreme",
        }
    }
}

/// The full desired device state. Held in `AppModel`, rebuilt from `Config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mode: ControlMode,
    /// Last preset chosen, retained across a Manual/Unmanaged detour so the
    /// UI can restore it. Invariant: when `mode == ControlMode::Preset(p)`,
    /// `preset == p`. `resolve()` reads the payload out of `mode`, never
    /// this field.
    pub preset: Preset,
    pub manual_fan_duty: u8,
    pub manual_pump_mode: PumpMode,
    /// Whether LiquidMon may write on its own when it observes the cooler
    /// has lost the setting. Never permits more than one write per run.
    pub auto_reapply: bool,
}

impl Settings {
    /// Resolve to the concrete (fan action, pump mode) pair to send, or
    /// `None` when the mode is `Unmanaged`. `manual_fan_duty` is clamped to
    /// `[MIN_FAN_DUTY, MAX_FAN_DUTY]` here, so an out-of-range persisted
    /// config can never reach the device.
    pub fn resolve(&self) -> Option<(FanAction, PumpMode)> {
        match self.mode {
            ControlMode::Unmanaged => None,
            ControlMode::Preset(preset) => {
                let pump = preset_pump_mode(preset);
                match preset.curve() {
                    Some(points) => Some((FanAction::Curve(points), pump)),
                    // Only `Max` has no curve: the intent is a flat wall, and
                    // the fixed-duty path is the one the driver models directly.
                    None => Some((FanAction::Fixed(MAX_FAN_DUTY), pump)),
                }
            }
            ControlMode::Manual => {
                let duty = self.manual_fan_duty.clamp(MIN_FAN_DUTY, MAX_FAN_DUTY);
                Some((FanAction::Fixed(duty), self.manual_pump_mode))
            }
        }
    }

    /// Stable fingerprint of everything that reaches the device. Written to
    /// the per-boot marker so a restart within the same boot can skip the
    /// divergence check entirely. Excludes `auto_reapply`, which changes no
    /// device state.
    ///
    /// Hashed through `std::hash::DefaultHasher`, whose output is explicitly
    /// not stable across Rust releases — fine here, since the marker lives
    /// in tmpfs for one boot and a toolchain change between two runs in the
    /// same boot at worst costs one redundant divergence check.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        if let Some((fan_action, pump_mode)) = self.resolve() {
            match fan_action {
                FanAction::Fixed(duty) => {
                    0u8.hash(&mut hasher);
                    duty.hash(&mut hasher);
                }
                FanAction::Curve(points) => {
                    1u8.hash(&mut hasher);
                    points.hash(&mut hasher);
                }
            }
            pump_mode.as_arg().hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// What to write to the `fan` channel: a flat duty, or a device-side curve
/// of `(liquid °C, duty %)` points the cooler keeps following on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanAction {
    Fixed(u8),
    Curve(&'static [(u8, u8)]),
}

/// Why an apply is being dispatched. Only `User` bypasses the per-run
/// budget and the marker fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTrigger {
    User,
    Divergence,
}

/// What the status line under the controls says. Carries the session write
/// count so the line doubles as a receipt for the write policy.
///
/// The timestamp is an `Instant`, rendered relative ("just now", "4 min
/// ago"), not a wall-clock time: `std` cannot format local time and this
/// feature does not justify pulling in a date/time crate.
#[derive(Debug, Default, Clone)]
pub enum ApplyStatus {
    #[default]
    Never,
    Ok {
        at: Instant,
        writes: u32,
    },
    Failed {
        stderr: String,
        writes: u32,
    },
}

/// The preset's points exactly as `apply_steps` sends them (no failsafe).
pub fn curve_points(preset: Preset) -> Option<&'static [(u8, u8)]> {
    preset.curve()
}

/// The curve as the DEVICE runs it: `curve_points` plus `CURVE_FAILSAFE`.
/// Prediction and the UI preview both use this so they cannot disagree with
/// what liquidctl actually programmed.
pub fn effective_curve(preset: Preset) -> Option<Vec<(u8, u8)>> {
    let points = curve_points(preset)?;
    let mut curve = points.to_vec();
    curve.push(CURVE_FAILSAFE);
    Some(curve)
}

/// Linear interpolation of `points` at `liquid_temp_c`; flat below the
/// first point and above the last.
///
/// # Panics
///
/// Panics if `points` is empty. Callers only ever pass preset curves or
/// `effective_curve`'s output, both non-empty by construction.
pub fn curve_duty_at(points: &[(u8, u8)], liquid_temp_c: f64) -> f64 {
    let (first_t, first_d) = points[0];
    let (last_t, last_d) = points[points.len() - 1];
    if liquid_temp_c <= f64::from(first_t) {
        return f64::from(first_d);
    }
    if liquid_temp_c >= f64::from(last_t) {
        return f64::from(last_d);
    }
    points
        .windows(2)
        .find_map(|pair| {
            let (t0, d0) = pair[0];
            let (t1, d1) = pair[1];
            let (t0, t1) = (f64::from(t0), f64::from(t1));
            if liquid_temp_c < t0 || liquid_temp_c > t1 {
                return None;
            }
            let frac = (liquid_temp_c - t0) / (t1 - t0);
            Some(f64::from(d0) + frac * (f64::from(d1) - f64::from(d0)))
        })
        .unwrap_or(f64::from(last_d))
}

/// Fan duty the active setting should be producing at `liquid_temp_c`: the
/// fixed duty for `Manual`/`Max`, or the preset's `effective_curve`
/// interpolated via `curve_duty_at`. Using the effective curve (which
/// includes liquidctl's failsafe point) matters above the preset's last
/// point — e.g. a `Balanced` device above 45 °C ramps from 90% toward the
/// failsafe's 100%, it does not hold at 90%. `None` for `Unmanaged`.
pub fn expected_fan_duty(settings: &Settings, liquid_temp_c: f64) -> Option<f64> {
    match settings.mode {
        ControlMode::Unmanaged => None,
        ControlMode::Preset(Preset::Max) => Some(f64::from(MAX_FAN_DUTY)),
        ControlMode::Preset(preset) => {
            let curve = effective_curve(preset)?;
            Some(curve_duty_at(&curve, liquid_temp_c))
        }
        ControlMode::Manual => Some(f64::from(
            settings.manual_fan_duty.clamp(MIN_FAN_DUTY, MAX_FAN_DUTY),
        )),
    }
}

/// Mean reported fan duty. `fans` is a handful of channels at most, so the
/// `usize -> f64` conversion loses no precision in practice.
#[allow(clippy::cast_precision_loss)]
fn fan_duty_mean(fans: &[Fan]) -> f64 {
    let sum: f64 = fans.iter().map(|f| f64::from(f.duty_pct)).sum();
    sum / (fans.len() as f64)
}

/// Whether a status sample contradicts the active setting, within the
/// tolerance for its kind (2 points fixed, 6 points curve — mean of all
/// reported fans). `None` when there is nothing to compare: `Unmanaged`, or
/// a sample with no fans. Classifies a single sample; requiring two
/// consecutive diverging samples before acting is the caller's job.
pub fn diverges(settings: &Settings, status: &AioStatus) -> Option<bool> {
    if status.fans.is_empty() {
        return None;
    }
    let (fan_action, _) = settings.resolve()?;
    match fan_action {
        FanAction::Fixed(target) => {
            let target = f64::from(target);
            Some(
                status
                    .fans
                    .iter()
                    .all(|fan| (f64::from(fan.duty_pct) - target).abs() > 2.0),
            )
        }
        FanAction::Curve(_) => {
            let expected = expected_fan_duty(settings, status.liquid_temp_c)?;
            let mean = fan_duty_mean(&status.fans);
            Some((mean - expected).abs() > 6.0)
        }
    }
}

/// Build the ordered argv lists for one apply. Fans first, pump second —
/// see "payload-clobber trap": a per-fan write after a fresh boot would
/// slam every other fan to 100%, so `fan` (not `fanN`) is the only channel
/// ever written, and pump mode is only settable via the `initialize` call
/// that re-sends the whole payload after it. Returns `None` for
/// `Unmanaged` or an uncontrollable device.
pub fn apply_steps(
    match_str: &str,
    settings: &Settings,
    cap: Capability,
) -> Option<Vec<Vec<String>>> {
    if cap != Capability::FanDutyAndPumpMode {
        return None;
    }
    let (fan_action, pump_mode) = settings.resolve()?;

    let mut fan_step = vec![
        "--match".to_string(),
        match_str.to_string(),
        "set".to_string(),
        "fan".to_string(),
        "speed".to_string(),
    ];
    match fan_action {
        FanAction::Fixed(duty) => fan_step.push(duty.to_string()),
        FanAction::Curve(points) => {
            for (temp, duty) in points {
                fan_step.push(temp.to_string());
                fan_step.push(duty.to_string());
            }
        }
    }

    let pump_step = vec![
        "--match".to_string(),
        match_str.to_string(),
        "initialize".to_string(),
        "--pump-mode".to_string(),
        pump_mode.as_arg().to_string(),
    ];

    Some(vec![fan_step, pump_step])
}

/// Directory holding the per-boot apply marker: `$XDG_RUNTIME_DIR/liquidmon`.
/// `None` when `XDG_RUNTIME_DIR` is unset, which degrades to "no fast path"
/// rather than falling back to a persistent location — the marker must die
/// with the boot, or it would suppress a needed write after a power cut.
pub fn marker_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("liquidmon"))
}

/// True when this boot already applied `settings` to `description`. Takes
/// the directory rather than reading the environment so the tests can point
/// it at a temp dir — `std::env::set_var` is `unsafe` in edition 2024 and is
/// not worth it for this.
pub fn marker_matches(dir: &Path, description: &str, settings: &Settings) -> bool {
    let Ok(contents) = std::fs::read_to_string(dir.join("applied")) else {
        return false;
    };
    let Some((marked_description, marked_fingerprint)) = contents.split_once('\n') else {
        return false;
    };
    let Ok(marked_fingerprint) = marked_fingerprint.trim().parse::<u64>() else {
        return false;
    };
    marked_description == description && marked_fingerprint == settings.fingerprint()
}

/// Record a successful apply. Best-effort: a failure to write the marker
/// costs one redundant divergence check next run, nothing more.
pub fn write_marker(dir: &Path, description: &str, settings: &Settings) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let contents = format!("{description}\n{}", settings.fingerprint());
    let _ = std::fs::write(dir.join("applied"), contents);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liquidctl::Pump;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn settings(mode: ControlMode) -> Settings {
        Settings {
            mode,
            preset: Preset::Balanced,
            manual_fan_duty: 50,
            manual_pump_mode: PumpMode::Balanced,
            auto_reapply: true,
        }
    }

    fn fans(duties: &[u8]) -> Vec<Fan> {
        duties
            .iter()
            .enumerate()
            .map(|(i, &duty_pct)| Fan {
                index: u8::try_from(i).unwrap_or(0) + 1,
                speed_rpm: 1200,
                duty_pct,
            })
            .collect()
    }

    fn status_with(liquid_temp_c: f64, duties: &[u8]) -> AioStatus {
        AioStatus {
            description: "Corsair Hydro H150i Pro XT".to_string(),
            liquid_temp_c,
            pump: Pump {
                speed_rpm: 2000,
                duty_pct: 50,
            },
            fans: fans(duties),
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    fn unique_marker_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "liquidmon-control-test-{tag}-{}-{n}",
            std::process::id()
        ))
    }

    fn assert_curve_is_valid(points: &[(u8, u8)]) {
        assert!(points.len() <= 6, "at most 6 points, got {}", points.len());
        for pair in points.windows(2) {
            let (t0, d0) = pair[0];
            let (t1, d1) = pair[1];
            assert!(t1 > t0, "temperatures must strictly increase: {t0} -> {t1}");
            assert!(d1 >= d0, "duty must be non-decreasing: {d0} -> {d1}");
        }
        for &(_, duty) in points {
            assert!(
                (MIN_FAN_DUTY..=MAX_FAN_DUTY).contains(&duty),
                "duty {duty} outside [{MIN_FAN_DUTY}, {MAX_FAN_DUTY}]"
            );
        }
    }

    #[test]
    fn silent_curve_is_monotonic_and_within_six_points() {
        assert_curve_is_valid(Preset::Silent.curve().expect("silent has a curve"));
    }

    #[test]
    fn balanced_curve_is_monotonic_and_within_six_points() {
        assert_curve_is_valid(Preset::Balanced.curve().expect("balanced has a curve"));
    }

    #[test]
    fn performance_curve_is_monotonic_and_within_six_points() {
        assert_curve_is_valid(
            Preset::Performance
                .curve()
                .expect("performance has a curve"),
        );
    }

    #[test]
    fn max_preset_has_no_curve() {
        assert_eq!(Preset::Max.curve(), None);
        assert_eq!(curve_points(Preset::Max), None);
        assert_eq!(effective_curve(Preset::Max), None);
    }

    #[test]
    fn capability_recognizes_all_twelve_hydro_platinum_descriptions() {
        let descriptions = [
            "Corsair Hydro H60i Pro XT",
            "Corsair Hydro H100i Pro XT",
            "Corsair Hydro H115i Pro XT",
            "Corsair Hydro H150i Pro XT",
            "Corsair Hydro H100i Platinum",
            "Corsair Hydro H100i Platinum SE",
            "Corsair Hydro H115i Platinum",
            "Corsair iCUE H100i Elite RGB",
            "Corsair iCUE H115i Elite RGB",
            "Corsair iCUE H150i Elite RGB",
            "Corsair iCUE H100i Elite RGB (White)",
            "Corsair iCUE H150i Elite RGB (White)",
        ];
        for description in descriptions {
            assert_eq!(
                capability(description),
                Capability::FanDutyAndPumpMode,
                "{description} should be controllable"
            );
        }
    }

    #[test]
    fn capability_rejects_asetek_pro_commander_core_and_non_aio() {
        assert_eq!(capability("Corsair Hydro H150i Pro"), Capability::None);
        assert_eq!(
            capability("Corsair Commander Core (broken)"),
            Capability::None
        );
        assert_eq!(
            capability("Corsair RMi Series Power Supply"),
            Capability::None
        );
    }

    #[test]
    fn apply_steps_matches_balanced_curve_argv_exactly() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        let steps = apply_steps(
            "Corsair Hydro H150i Pro XT",
            &s,
            Capability::FanDutyAndPumpMode,
        )
        .expect("balanced preset resolves");
        assert_eq!(
            steps,
            vec![
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "set",
                    "fan",
                    "speed",
                    "25",
                    "25",
                    "30",
                    "35",
                    "35",
                    "50",
                    "40",
                    "70",
                    "45",
                    "90",
                ]),
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "initialize",
                    "--pump-mode",
                    "balanced",
                ]),
            ]
        );
    }

    #[test]
    fn apply_steps_matches_max_fixed_duty_argv() {
        let s = settings(ControlMode::Preset(Preset::Max));
        let steps = apply_steps(
            "Corsair Hydro H150i Pro XT",
            &s,
            Capability::FanDutyAndPumpMode,
        )
        .expect("max preset resolves");
        assert_eq!(
            steps,
            vec![
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "set",
                    "fan",
                    "speed",
                    "100"
                ]),
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "initialize",
                    "--pump-mode",
                    "extreme",
                ]),
            ]
        );
    }

    #[test]
    fn apply_steps_matches_manual_argv() {
        let mut s = settings(ControlMode::Manual);
        s.manual_fan_duty = 55;
        s.manual_pump_mode = PumpMode::Quiet;
        let steps = apply_steps(
            "Corsair Hydro H150i Pro XT",
            &s,
            Capability::FanDutyAndPumpMode,
        )
        .expect("manual resolves");
        assert_eq!(
            steps,
            vec![
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "set",
                    "fan",
                    "speed",
                    "55"
                ]),
                argv(&[
                    "--match",
                    "Corsair Hydro H150i Pro XT",
                    "initialize",
                    "--pump-mode",
                    "quiet",
                ]),
            ]
        );
    }

    #[test]
    fn apply_steps_writes_fans_before_pump_mode() {
        let s = settings(ControlMode::Preset(Preset::Silent));
        let steps = apply_steps(
            "Corsair Hydro H150i Pro XT",
            &s,
            Capability::FanDutyAndPumpMode,
        )
        .expect("silent preset resolves");
        assert_eq!(steps.len(), 2);
        assert!(
            steps[0].contains(&"fan".to_string()),
            "first step must write the fan channel"
        );
        assert!(
            steps[1].contains(&"initialize".to_string()),
            "second step must be the pump-mode initialize call"
        );
    }

    #[test]
    fn apply_steps_is_none_for_unmanaged_and_uncontrollable_device() {
        let unmanaged = settings(ControlMode::Unmanaged);
        assert_eq!(
            apply_steps(
                "Corsair Hydro H150i Pro XT",
                &unmanaged,
                Capability::FanDutyAndPumpMode
            ),
            None
        );
        let managed = settings(ControlMode::Preset(Preset::Balanced));
        assert_eq!(
            apply_steps("Corsair Hydro H150i Pro", &managed, Capability::None),
            None
        );
    }

    #[test]
    fn manual_duty_clamps_at_both_ends() {
        let mut s = settings(ControlMode::Manual);
        s.manual_fan_duty = 5;
        let (fan_action, _) = s.resolve().expect("manual resolves");
        assert_eq!(fan_action, FanAction::Fixed(MIN_FAN_DUTY));

        s.manual_fan_duty = 255;
        let (fan_action, _) = s.resolve().expect("manual resolves");
        assert_eq!(fan_action, FanAction::Fixed(MAX_FAN_DUTY));
    }

    #[test]
    fn apply_steps_never_emits_non_volatile() {
        let modes = [
            ControlMode::Unmanaged,
            ControlMode::Preset(Preset::Silent),
            ControlMode::Preset(Preset::Balanced),
            ControlMode::Preset(Preset::Performance),
            ControlMode::Preset(Preset::Max),
            ControlMode::Manual,
        ];
        for mode in modes {
            let s = settings(mode);
            if let Some(steps) = apply_steps(
                "Corsair Hydro H150i Pro XT",
                &s,
                Capability::FanDutyAndPumpMode,
            ) {
                for step in steps {
                    assert!(
                        !step.iter().any(|arg| arg == "--non-volatile"),
                        "{mode:?} must never pass --non-volatile"
                    );
                }
            }
        }
    }

    #[test]
    fn expected_fan_duty_interpolates_between_curve_points() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        // Balanced: (30,35) .. (35,50); halfway at 32.5C should be 42.5%.
        let duty = expected_fan_duty(&s, 32.5).expect("balanced resolves");
        assert!((duty - 42.5).abs() < 1e-9, "got {duty}");
    }

    #[test]
    fn expected_fan_duty_is_flat_outside_curve_bounds() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        assert_eq!(expected_fan_duty(&s, 10.0), Some(25.0));
        assert_eq!(expected_fan_duty(&s, 70.0), Some(100.0));
    }

    #[test]
    fn expected_fan_duty_ramps_past_last_preset_point_toward_failsafe() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        // Between the last preset point (45, 90) and the failsafe (60, 100)
        // it must keep climbing, not hold flat at 90.
        let duty = expected_fan_duty(&s, 50.0).expect("balanced resolves");
        assert!(duty > 90.0, "expected duty above 90 at 50C, got {duty}");
    }

    #[test]
    fn expected_fan_duty_is_fixed_for_manual_and_max() {
        let mut manual = settings(ControlMode::Manual);
        manual.manual_fan_duty = 65;
        assert_eq!(expected_fan_duty(&manual, 30.0), Some(65.0));

        let max = settings(ControlMode::Preset(Preset::Max));
        assert_eq!(expected_fan_duty(&max, 55.0), Some(100.0));
    }

    #[test]
    fn diverges_is_false_within_fixed_duty_tolerance() {
        let s = settings(ControlMode::Preset(Preset::Max));
        let status = status_with(35.0, &[99, 100, 98]);
        assert_eq!(diverges(&s, &status), Some(false));
    }

    #[test]
    fn diverges_is_true_past_fixed_duty_tolerance() {
        let s = settings(ControlMode::Preset(Preset::Max));
        let status = status_with(35.0, &[80, 82, 79]);
        assert_eq!(diverges(&s, &status), Some(true));
    }

    #[test]
    fn diverges_is_false_within_curve_tolerance() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        // Expected duty at 32.5C is 42.5%; mean of these two is within 6.
        let status = status_with(32.5, &[40, 45]);
        assert_eq!(diverges(&s, &status), Some(false));
    }

    #[test]
    fn diverges_is_true_past_curve_tolerance() {
        let s = settings(ControlMode::Preset(Preset::Balanced));
        let status = status_with(32.5, &[60, 65]);
        assert_eq!(diverges(&s, &status), Some(true));
    }

    #[test]
    fn diverges_is_none_for_unmanaged_and_fanless_sample() {
        let unmanaged = settings(ControlMode::Unmanaged);
        let status = status_with(35.0, &[50]);
        assert_eq!(diverges(&unmanaged, &status), None);

        let managed = settings(ControlMode::Preset(Preset::Balanced));
        let fanless = status_with(35.0, &[]);
        assert_eq!(diverges(&managed, &fanless), None);
    }

    #[test]
    fn fingerprint_changes_with_fan_action_and_pump_mode_not_auto_reapply() {
        let mut a = settings(ControlMode::Manual);
        a.manual_fan_duty = 50;
        a.manual_pump_mode = PumpMode::Balanced;

        let mut b = a.clone();
        b.manual_fan_duty = 60;
        assert_ne!(
            a.fingerprint(),
            b.fingerprint(),
            "fan duty change must change fingerprint"
        );

        let mut c = a.clone();
        c.manual_pump_mode = PumpMode::Extreme;
        assert_ne!(
            a.fingerprint(),
            c.fingerprint(),
            "pump mode change must change fingerprint"
        );

        let mut d = a.clone();
        d.auto_reapply = !a.auto_reapply;
        assert_eq!(
            a.fingerprint(),
            d.fingerprint(),
            "auto_reapply must not affect fingerprint"
        );
    }

    #[test]
    fn marker_round_trip_matches_written_settings() {
        let dir = unique_marker_dir("round-trip");
        let s = settings(ControlMode::Preset(Preset::Balanced));
        write_marker(&dir, "Corsair Hydro H150i Pro XT", &s);
        assert!(marker_matches(&dir, "Corsair Hydro H150i Pro XT", &s));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marker_mismatches_different_device_or_fingerprint() {
        let dir = unique_marker_dir("mismatch");
        let s = settings(ControlMode::Preset(Preset::Balanced));
        write_marker(&dir, "Corsair Hydro H150i Pro XT", &s);

        assert!(!marker_matches(&dir, "Corsair Hydro H100i Pro XT", &s));

        let mut other = s.clone();
        other.mode = ControlMode::Preset(Preset::Silent);
        assert!(!marker_matches(&dir, "Corsair Hydro H150i Pro XT", &other));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marker_missing_directory_returns_false_without_panic() {
        let dir = unique_marker_dir("missing");
        let s = settings(ControlMode::Preset(Preset::Balanced));
        assert!(!marker_matches(&dir, "Corsair Hydro H150i Pro XT", &s));
    }
}
