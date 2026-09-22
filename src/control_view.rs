// SPDX-License-Identifier: MPL-2.0

//! The popup's cooling-control section: mode dropdown, preset curve preview,
//! manual fan/pump controls, the write-policy row and the apply-status line.
//! Its own module rather than another 180 lines in `view.rs`; like those
//! builders it takes plain data from `app.rs` and borrows no model state
//! beyond the two values that cannot be copied (`ApplyStatus`, `pump_model`).

use crate::app::Message;
use crate::control::{self, ControlMode, Preset};
use crate::curve::CurvePreview;
use cosmic::iced::widget::canvas::Canvas;
use cosmic::iced::widget::{Space, column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::widget::segmented_button;
use std::time::{Duration, Instant};

/// Dropdown entries, in menu order. `widget::dropdown` renders flat labels
/// only, so the caption *under* the dropdown carries each mode's explanation.
const MODES: [ControlMode; 6] = [
    ControlMode::Unmanaged,
    ControlMode::Preset(Preset::Silent),
    ControlMode::Preset(Preset::Balanced),
    ControlMode::Preset(Preset::Performance),
    ControlMode::Preset(Preset::Max),
    ControlMode::Manual,
];

/// Size of the curve preview canvas, matching the popup's content width.
const CURVE_SIZE: (f32, f32) = (348.0, 46.0);
/// Temperature bounds of the preview frame, fixed so two presets never look
/// alike — the same absolute-range argument the equalizer makes.
const CURVE_TEMP_RANGE: (u8, u8) = (20, 60);
/// Slider granularity for the manual fan duty, in percentage points.
const FAN_DUTY_STEP: f32 = 5.0;

const UNMANAGED_CAPTION: &str = "Not writing to this cooler. The readouts above are whatever it is already running — set by BIOS, another tool, or its own defaults.";
const READ_ONLY_CAPTION: &str =
    "No verified liquidctl write path for this device — LiquidMon only reads it.";
const PUMP_MODE_CAPTION: &str =
    "This family has no pump duty — three modes is all the firmware exposes.";
const REAPPLY_LABEL: &str = "Re-apply if the cooler loses it";

/// Which body the section renders under the dropdown. A value rather than
/// inline branching so the choice is unit-testable without an `Element`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    /// Not writing: one explanatory caption.
    Unmanaged,
    /// A device-side curve: preview canvas plus its marker labels.
    Curve(Preset),
    /// A preset with no curve (`Max`): caption only.
    Fixed,
    /// Manual: fan slider and pump segments.
    Manual,
}

/// Cooling control: a mode dropdown (Unmanaged / four presets / Manual), the
/// active preset's curve preview, the manual fan slider and pump segments,
/// the write-policy row, and the apply-status line. Renders a read-only
/// caption when the device has no verified write path.
pub(crate) fn control_section<'a>(
    settings: &control::Settings,
    capability: control::Capability,
    pending_fan_duty: Option<f32>,
    liquid_temp_c: f64,
    apply_in_flight: bool,
    last_apply: &'a control::ApplyStatus,
    pump_model: &'a segmented_button::SingleSelectModel,
) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = Vec::new();

    if let Some(caption) = read_only_caption(capability) {
        parts.push(header(None));
        parts.push(widget::text::caption(caption).into());
        return cosmic::iced::widget::Column::with_children(parts)
            .spacing(6)
            .into();
    }

    parts.push(header(Some(mode_label(settings.mode))));
    parts.push(mode_dropdown(settings.mode));
    match body_for(settings.mode) {
        Body::Unmanaged | Body::Fixed => {}
        Body::Curve(preset) => parts.extend(
            control::effective_curve(preset).map(|points| curve_body(&points, liquid_temp_c)),
        ),
        Body::Manual => parts.push(manual_body(settings, pending_fan_duty, pump_model)),
    }
    parts.extend(mode_caption(settings).map(|text| widget::text::caption(text).into()));

    // Unmanaged writes nothing, so it carries neither the write-policy row nor
    // a status line.
    if !matches!(settings.mode, ControlMode::Unmanaged) {
        parts.push(policy_row(settings.auto_reapply, apply_in_flight));
        if apply_in_flight {
            parts.push(widget::text::caption("Applying…").into());
        } else {
            parts.extend(
                status_text(last_apply, Instant::now())
                    .map(|text| status_caption(last_apply, text)),
            );
        }
    }

    cosmic::iced::widget::Column::with_children(parts)
        .spacing(6)
        .into()
}

/// Section header: the small-caps label, plus the active mode's name in accent
/// mono when the device is controllable at all.
fn header<'a>(active_mode: Option<&'static str>) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = vec![
        widget::text::caption("COOLING").into(),
        Space::new().width(Length::Fill).into(),
    ];
    items.extend(active_mode.map(|label| {
        widget::text::body(label)
            .font(cosmic::font::mono())
            .class(cosmic::theme::Text::Accent)
            .into()
    }));
    cosmic::iced::widget::Row::with_children(items)
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// The six-entry mode picker. The `on_selected` closure must be `'static`, so
/// it cannot borrow the entry list: the modes are moved in and indexed there,
/// the same shape as `view::device_dropdown_section`.
fn mode_dropdown<'a>(mode: ControlMode) -> Element<'a, Message> {
    let entries = mode_entries();
    let labels: Vec<String> = entries
        .iter()
        .map(|(label, _)| (*label).to_string())
        .collect();
    let modes: Vec<ControlMode> = entries.into_iter().map(|(_, entry)| entry).collect();
    widget::dropdown(labels, mode_selected_index(mode), move |idx: usize| {
        Message::ControlModeSelected(modes.get(idx).copied().unwrap_or(ControlMode::Unmanaged))
    })
    .into()
}

/// Curve preview: the polyline the device is actually running, with the frame
/// bounds and the live marker read out beneath it.
fn curve_body<'a>(points: &[(u8, u8)], liquid_temp_c: f64) -> Element<'a, Message> {
    let duty = control::curve_duty_at(points, liquid_temp_c);
    let preview = Canvas::new(CurvePreview::new(points.to_vec(), liquid_temp_c))
        .width(Length::Fixed(CURVE_SIZE.0))
        .height(Length::Fixed(CURVE_SIZE.1));
    let labels = row![
        widget::text::caption(format!("{} °C", CURVE_TEMP_RANGE.0)).font(cosmic::font::mono()),
        Space::new().width(Length::Fill),
        widget::text::caption(format!("{liquid_temp_c:.0} °C · {duty:.0}%"))
            .font(cosmic::font::mono()),
        Space::new().width(Length::Fill),
        widget::text::caption(format!("{} °C", CURVE_TEMP_RANGE.1)).font(cosmic::font::mono()),
    ];
    column![preview, labels].spacing(4).into()
}

/// Manual body: the all-fan duty slider with its floor caption, then the
/// three pump modes with theirs.
fn manual_body<'a>(
    settings: &control::Settings,
    pending_fan_duty: Option<f32>,
    pump_model: &'a segmented_button::SingleSelectModel,
) -> Element<'a, Message> {
    // Live drag value falls back to the persisted setting when not dragging,
    // exactly like the sample-interval slider.
    let duty = pending_fan_duty.unwrap_or(f32::from(settings.manual_fan_duty));
    let fan_header = row![
        widget::text::caption("FAN DUTY"),
        Space::new().width(Length::Fill),
        widget::text::body(format!("{duty:.0} %")).font(cosmic::font::mono()),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let slider = widget::slider(
        f32::from(control::MIN_FAN_DUTY)..=f32::from(control::MAX_FAN_DUTY),
        duty,
        Message::ManualFanDragged,
    )
    .step(FAN_DUTY_STEP)
    .on_release(Message::ManualFanReleased)
    .width(Length::Fill);

    column![
        fan_header,
        slider,
        widget::text::caption(format!(
            "Floor is {}% — most 120 mm fans stall below that.",
            control::MIN_FAN_DUTY
        )),
        widget::text::caption("PUMP"),
        pump_segments(pump_model),
        widget::text::caption(PUMP_MODE_CAPTION),
    ]
    .spacing(6)
    .into()
}

/// The three pump modes as a segmented control. `on_activate` must be
/// `'static`, so it cannot borrow the model: the entity → mode pairs are read
/// off it here and carried into the closure.
fn pump_segments(pump_model: &segmented_button::SingleSelectModel) -> Element<'_, Message> {
    let modes: Vec<(segmented_button::Entity, control::PumpMode)> = pump_model
        .iter()
        .filter_map(|entity| {
            pump_model
                .data::<control::PumpMode>(entity)
                .map(|mode| (entity, *mode))
        })
        .collect();
    widget::segmented_control::horizontal(pump_model)
        .on_activate(move |entity| {
            let mode = modes.iter().find(|(e, _)| *e == entity).map(|(_, m)| *m);
            Message::ManualPumpSelected(mode.unwrap_or_default())
        })
        .into()
}

/// The write-policy row: the automatic re-apply toggle and the manual apply
/// button. `on_press_maybe(None)` is how a standard button renders disabled.
fn policy_row<'a>(auto_reapply: bool, apply_in_flight: bool) -> Element<'a, Message> {
    let toggle = widget::checkbox(auto_reapply)
        .label(REAPPLY_LABEL)
        .on_toggle(Message::AutoReapplyToggled);
    let apply = widget::button::standard("Apply now")
        .on_press_maybe((!apply_in_flight).then_some(Message::ApplyRequested));
    row![toggle, Space::new().width(Length::Fill), apply]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// The apply-status line. A failed write is the one caption drawn in the
/// theme's destructive colour; everything else keeps the default.
fn status_caption<'a>(status: &control::ApplyStatus, text: String) -> Element<'a, Message> {
    let caption = widget::text::caption(text).font(cosmic::font::mono());
    match status {
        control::ApplyStatus::Failed { .. } => caption
            .class(cosmic::theme::Text::Custom(|theme| {
                cosmic::iced::widget::text::Style {
                    color: Some(theme.cosmic().destructive_color().into()),
                }
            }))
            .into(),
        _ => caption.into(),
    }
}

/// Caption that replaces every control when the device's family has no
/// verified write path. `None` when the device is controllable.
pub(crate) fn read_only_caption(capability: control::Capability) -> Option<&'static str> {
    match capability {
        control::Capability::None => Some(READ_ONLY_CAPTION),
        control::Capability::FanDutyAndPumpMode => None,
    }
}

/// Label for one dropdown entry.
pub(crate) fn mode_label(mode: ControlMode) -> &'static str {
    match mode {
        ControlMode::Unmanaged => "Unmanaged",
        ControlMode::Preset(Preset::Silent) => "Silent",
        ControlMode::Preset(Preset::Balanced) => "Balanced",
        ControlMode::Preset(Preset::Performance) => "Performance",
        ControlMode::Preset(Preset::Max) => "Max",
        ControlMode::Manual => "Manual",
    }
}

/// The dropdown entries as `(label, mode)` pairs, in menu order.
pub(crate) fn mode_entries() -> Vec<(&'static str, ControlMode)> {
    MODES.iter().map(|m| (mode_label(*m), *m)).collect()
}

/// Index of the active mode in the dropdown.
pub(crate) fn mode_selected_index(mode: ControlMode) -> Option<usize> {
    MODES.iter().position(|m| *m == mode)
}

/// Body the section renders for `mode`. Whether a preset draws a curve is
/// read from the preset table itself, so the two can never disagree.
pub(crate) fn body_for(mode: ControlMode) -> Body {
    match mode {
        ControlMode::Unmanaged => Body::Unmanaged,
        ControlMode::Manual => Body::Manual,
        ControlMode::Preset(preset) => match control::curve_points(preset) {
            Some(_) => Body::Curve(preset),
            None => Body::Fixed,
        },
    }
}

/// Caption under the dropdown describing what the selected mode does. `None`
/// for Manual, whose body carries its own two captions.
pub(crate) fn mode_caption(settings: &control::Settings) -> Option<String> {
    let pump = settings.resolve().map(|(_, pump)| pump);
    match body_for(settings.mode) {
        Body::Unmanaged => Some(UNMANAGED_CAPTION.to_string()),
        Body::Curve(preset) => Some(format!(
            "{} points written into the cooler. It keeps following them with LiquidMon closed. Pump: {}.",
            control::effective_curve(preset)?.len(),
            pump?.as_arg(),
        )),
        Body::Fixed => Some(format!(
            "Fans pinned at {}%, pump on {}. No curve — the cooler holds this until you change it.",
            control::MAX_FAN_DUTY,
            pump?.as_arg(),
        )),
        Body::Manual => None,
    }
}

/// Text of the apply-status line, or `None` for `ApplyStatus::Never` — a
/// section that has never written shows no line at all. `now` is threaded in
/// so the relative age is testable without sleeping.
pub(crate) fn status_text(status: &control::ApplyStatus, now: Instant) -> Option<String> {
    match status {
        control::ApplyStatus::Never => None,
        control::ApplyStatus::Ok { at, writes } => Some(format!(
            "Written {} · {}",
            relative_age(now.saturating_duration_since(*at)),
            writes_phrase(*writes),
        )),
        control::ApplyStatus::Failed { stderr, .. } => Some(format!("error: {stderr}")),
    }
}

/// Age of the last write, rendered relative rather than as a wall clock:
/// `std` cannot format local time and this feature does not justify a
/// date/time crate. The popup's 33 ms animation tick re-renders while it is
/// open, so the label ages on its own.
fn relative_age(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else {
        format!("{} h ago", secs / 3600)
    }
}

/// `1 write this session` / `n writes this session`. The count is always
/// shown: the status line doubles as a receipt for the write policy.
fn writes_phrase(writes: u32) -> String {
    if writes == 1 {
        "1 write this session".to_string()
    } else {
        format!("{writes} writes this session")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: ControlMode) -> control::Settings {
        control::Settings {
            mode,
            preset: match mode {
                ControlMode::Preset(preset) => preset,
                _ => Preset::Balanced,
            },
            manual_fan_duty: 50,
            manual_pump_mode: control::PumpMode::Balanced,
            auto_reapply: true,
        }
    }

    #[test]
    fn mode_entries_are_in_menu_order() {
        let labels: Vec<&str> = mode_entries().into_iter().map(|(label, _)| label).collect();
        assert_eq!(
            labels,
            vec![
                "Unmanaged",
                "Silent",
                "Balanced",
                "Performance",
                "Max",
                "Manual"
            ],
        );
    }

    #[test]
    fn mode_selected_index_finds_the_active_preset() {
        assert_eq!(
            mode_selected_index(ControlMode::Preset(Preset::Performance)),
            Some(3),
        );
        assert_eq!(mode_selected_index(ControlMode::Unmanaged), Some(0));
        assert_eq!(mode_selected_index(ControlMode::Manual), Some(5));
    }

    #[test]
    fn body_per_mode_picks_curve_caption_or_manual_controls() {
        assert_eq!(body_for(ControlMode::Unmanaged), Body::Unmanaged);
        assert_eq!(
            body_for(ControlMode::Preset(Preset::Balanced)),
            Body::Curve(Preset::Balanced),
        );
        // Max is a flat fixed duty, not a curve — caption only.
        assert_eq!(body_for(ControlMode::Preset(Preset::Max)), Body::Fixed);
        assert_eq!(body_for(ControlMode::Manual), Body::Manual);
    }

    #[test]
    fn unsupported_device_renders_a_read_only_caption() {
        // `Capability::None` drops the dropdown entirely and the caption is
        // the section's whole body; a controllable device gets no caption.
        assert_eq!(
            read_only_caption(control::Capability::None),
            Some(READ_ONLY_CAPTION),
        );
        assert_eq!(
            read_only_caption(control::Capability::FanDutyAndPumpMode),
            None,
        );
    }

    #[test]
    fn curve_preset_caption_names_point_count_and_pump_mode() {
        let caption = mode_caption(&settings(ControlMode::Preset(Preset::Balanced)))
            .expect("a curve preset has a caption");
        let points = control::effective_curve(Preset::Balanced)
            .expect("Balanced is a curve preset")
            .len();
        assert!(
            caption.starts_with(&format!("{points} points written into the cooler")),
            "got {caption:?}",
        );
        assert!(caption.ends_with("Pump: balanced."), "got {caption:?}");
    }

    #[test]
    fn manual_mode_has_no_dropdown_caption() {
        // Manual's two captions sit with the widgets they explain.
        assert_eq!(mode_caption(&settings(ControlMode::Manual)), None);
    }

    #[test]
    fn status_text_is_absent_until_the_first_write() {
        assert_eq!(
            status_text(&control::ApplyStatus::Never, Instant::now()),
            None
        );
    }

    #[test]
    fn status_text_uses_singular_for_one_write() {
        let now = Instant::now();
        let status = control::ApplyStatus::Ok { at: now, writes: 1 };
        assert_eq!(
            status_text(&status, now).as_deref(),
            Some("Written just now · 1 write this session"),
        );
    }

    #[test]
    fn status_text_pluralizes_and_ages_the_timestamp() {
        let at = Instant::now();
        let now = at + Duration::from_secs(4 * 60 + 30);
        let status = control::ApplyStatus::Ok { at, writes: 3 };
        assert_eq!(
            status_text(&status, now).as_deref(),
            Some("Written 4 min ago · 3 writes this session"),
        );
    }

    #[test]
    fn status_text_surfaces_the_stderr_tail_of_a_failure() {
        let status = control::ApplyStatus::Failed {
            stderr: "liquidctl: no device matches the given filters".to_string(),
            writes: 2,
        };
        assert_eq!(
            status_text(&status, Instant::now()).as_deref(),
            Some("error: liquidctl: no device matches the given filters"),
        );
    }
}
