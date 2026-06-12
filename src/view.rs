// SPDX-License-Identifier: MPL-2.0

//! Stateless view layer for the applet popup and panel button. These helpers
//! construct iced widgets from plain data threaded in by `app.rs`; none of them
//! borrow `AppModel` directly, so the model's fields stay private. The
//! `cosmic::Application::view` / `view_window` trait methods remain in `app.rs`
//! and call into these builders.

use crate::app::Message;
use crate::devices;
use crate::equalizer::Equalizer;
use crate::liquidctl::DetectedDevice;
use crate::spinner::{Kind, Spinner};
use cosmic::iced::widget::canvas::Canvas;
use cosmic::iced::widget::{Space, column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget;
use std::collections::VecDeque;

pub(crate) const ICON_TEMP: &[u8] = include_bytes!("../resources/icons/temperature-symbolic.svg");
pub(crate) const ICON_SNOWFLAKE: &[u8] =
    include_bytes!("../resources/icons/snowflake-symbolic.svg");
pub(crate) const ICON_FAN: &[u8] = include_bytes!("../resources/icons/fan-symbolic.svg");
pub(crate) const ICON_PUMP: &[u8] = include_bytes!("../resources/icons/pump-symbolic.svg");

/// Absolute VU-meter ranges anchoring the equalizer's green/amber/red ramp to
/// real magnitude: coolant temperature in °C and duty cycle in %. (Auto-scaling
/// would paint the tallest bar red regardless of the actual value.)
pub(crate) const TEMP_RANGE: (f64, f64) = (20.0, 55.0);
pub(crate) const DUTY_RANGE: (f64, f64) = (0.0, 100.0);

pub(crate) fn symbolic_icon_sized(bytes: &'static [u8], size: u16) -> widget::icon::Icon {
    let mut handle = widget::icon::from_svg_bytes(bytes);
    handle.symbolic = true;
    widget::icon::icon(handle).size(size)
}

pub(crate) fn symbolic_icon(bytes: &'static [u8]) -> widget::icon::Icon {
    symbolic_icon_sized(bytes, 14)
}

/// The 80s graphic-EQ canvas for a metric history, sized to fill the popup
/// width so it tracks the popup's responsive width bounds. `range` is the
/// absolute (lo, hi) the meter normalises against.
pub(crate) fn eq_canvas(
    history: &VecDeque<f64>,
    height: f32,
    range: (f64, f64),
) -> Element<'_, Message> {
    Canvas::new(Equalizer::new(history.iter().copied(), range.0, range.1))
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .into()
}

/// One popup metric block: a header row (leading glyph, small-caps label,
/// right-aligned mono value) above the equalizer canvas. Extracted so
/// `popup_metrics_view` stays under the file's per-fn soft limit.
pub(crate) fn metric_block<'a>(
    glyph: Element<'a, Message>,
    label: &'a str,
    value: Element<'a, Message>,
    history: &'a VecDeque<f64>,
    range: (f64, f64),
) -> Element<'a, Message> {
    let header = row![
        glyph,
        widget::text::caption(label),
        Space::new().width(Length::Fill),
        value,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    column![header, eq_canvas(history, 64.0, range)]
        .spacing(6)
        .into()
}

/// Right-aligned mono value text at a given size, the shared style for every
/// metric header's numeric readout.
pub(crate) fn metric_value(text: String, size: f32) -> Element<'static, Message> {
    widget::text::body(text)
        .font(cosmic::font::mono())
        .size(size)
        .into()
}

/// Per-fan breakdown rows, indented under the FANS block. The shared mono
/// format string keeps the rpm and duty columns vertically aligned with the
/// caption rows above.
pub(crate) fn fan_rows(status: &crate::liquidctl::AioStatus) -> Element<'_, Message> {
    let row_fmt = |label: &str, rpm: u32, duty: u8| -> String {
        format!("{label:<6}{rpm:>5} rpm   {duty:>3} %")
    };
    let mut rows: Vec<Element<'_, Message>> = Vec::with_capacity(status.fans.len());
    for fan in &status.fans {
        rows.push(
            widget::text::caption(row_fmt(
                &format!("fan {}", fan.index),
                fan.speed_rpm,
                fan.duty_pct,
            ))
            .font(cosmic::font::mono())
            .into(),
        );
    }
    row![
        Space::new().width(Length::Fixed(8.0)),
        cosmic::iced::widget::Column::with_children(rows).spacing(2),
    ]
    .into()
}

/// A small animated fan/pump glyph canvas for a metric header, spinning at
/// a rate derived from `rpm` and the popup-scoped animation clock.
pub(crate) fn spinner_glyph(anim_t: f32, kind: Kind, rpm: u32) -> Element<'static, Message> {
    Canvas::new(Spinner::new(kind, rpm, anim_t))
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(22.0))
        .into()
}

/// Sample-interval control: a small-caps label with a right-aligned mono
/// readout above the slider.
pub(crate) fn interval_control(
    pending_interval_secs: Option<f32>,
    sample_interval_ms: u64,
) -> Element<'static, Message> {
    // Live drag value falls back to the persisted setting when not dragging.
    // The slider is f32; the persisted value is u64 ms — narrowing only
    // above ~16 million ms, which we clamp far below.
    #[allow(clippy::cast_precision_loss)]
    let secs = pending_interval_secs.unwrap_or((sample_interval_ms as f32) / 1000.0);

    let header = row![
        widget::text::caption("SAMPLE INTERVAL"),
        Space::new().width(Length::Fill),
        widget::text::body(format!("{secs:.1} s")).font(cosmic::font::mono()),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let slider = widget::slider(1.0..=10.0_f32, secs, Message::SampleIntervalDragged)
        .step(0.5_f32)
        .on_release(Message::SampleIntervalReleased)
        .width(Length::Fill);

    column![header, slider].spacing(6).into()
}

/// Build the dropdown entries as `(display label, commit value)` pairs.
/// Index 0 is `Auto (...)` whose commit value is `None`. Then each
/// connected AIO that isn't the auto-pick (the auto entry already names
/// it). Finally, an optional synthetic `<saved> (disconnected)` row when
/// the saved choice is truly absent from the full enumeration.
///
/// Carrying the real commit value alongside the label means selection
/// commits the device's actual description rather than reconstructing it
/// from the display string (so a description that itself ends in
/// " (disconnected)" round-trips intact).
pub(crate) fn dropdown_entries(
    detected_devices: &[DetectedDevice],
    device_match: Option<&str>,
) -> Vec<(String, Option<String>)> {
    let aios: Vec<&DetectedDevice> = devices::filter_aios(detected_devices);
    let auto_pick = devices::auto_select(detected_devices);
    let auto_label = match auto_pick {
        Some(d) => format!("Auto ({})", d.description),
        None => "Auto (no AIO detected)".to_string(),
    };

    let mut entries: Vec<(String, Option<String>)> = vec![(auto_label, None)];
    let auto_desc = auto_pick.map(|d| d.description.as_str());
    entries.extend(
        aios.iter()
            .filter(|d| Some(d.description.as_str()) != auto_desc)
            .map(|d| (d.description.clone(), Some(d.description.clone()))),
    );

    // Gate the synthetic disconnected row on the FULL device list, not the
    // AIO-filtered subset: `effective_match` polls a saved description
    // verbatim regardless of AIO classification, so a connected non-AIO
    // saved device must NOT be mislabeled "(disconnected)".
    if let Some(saved) = device_match
        && !detected_devices.iter().any(|d| d.description == saved)
    {
        entries.push((format!("{saved} (disconnected)"), Some(saved.to_string())));
    }
    entries
}

/// Build the device dropdown display labels: `Auto (...)` at index 0, then
/// each connected AIO that isn't the auto-pick, then optionally a synthetic
/// `<saved> (disconnected)` row if the saved choice isn't currently present.
/// The view path uses `dropdown_entries` directly (it needs the commit
/// values too); this label-only projection exists for the dropdown tests.
#[cfg(test)]
pub(crate) fn device_dropdown_items(
    detected_devices: &[DetectedDevice],
    device_match: Option<&str>,
) -> Vec<String> {
    dropdown_entries(detected_devices, device_match)
        .into_iter()
        .map(|(label, _)| label)
        .collect()
}

/// Resolve the currently-selected dropdown index. `None` (saved choice
/// is unset) maps to index 0 (Auto). A saved choice that matches the
/// auto-pick also maps to Auto, since the explicit row is intentionally
/// hidden from the list to avoid duplicating the device under both Auto
/// and a standalone entry. Otherwise the saved choice maps to its row
/// (connected or `(disconnected)` synthetic) by its real commit value.
pub(crate) fn device_dropdown_selected(
    detected_devices: &[DetectedDevice],
    device_match: Option<&str>,
    entries: &[(String, Option<String>)],
) -> Option<usize> {
    match device_match {
        None => Some(0),
        Some(saved) => {
            let auto_desc = devices::auto_select(detected_devices).map(|d| d.description.as_str());
            if auto_desc == Some(saved) {
                return Some(0);
            }
            entries
                .iter()
                .position(|(_, value)| value.as_deref() == Some(saved))
        }
    }
}

/// Build the device-selector section of the popup: a `Device` label
/// over a dropdown, or a status caption while detection is pending or
/// no AIO is connected.
pub(crate) fn device_dropdown_section(
    detected_devices: &[DetectedDevice],
    device_match: Option<&str>,
    device_scan_in_flight: bool,
) -> Element<'static, Message> {
    if detected_devices.is_empty() && device_match.is_none() {
        let caption = if device_scan_in_flight {
            "Detecting devices…"
        } else {
            "No supported AIO detected"
        };
        return column![
            widget::text::caption("DEVICE"),
            widget::text::caption(caption)
        ]
        .spacing(6)
        .into();
    }

    let entries = dropdown_entries(detected_devices, device_match);
    let selected = device_dropdown_selected(detected_devices, device_match, &entries);
    let labels: Vec<String> = entries.iter().map(|(label, _)| label.clone()).collect();
    // The dropdown's `on_selected` closure must be 'static + Send + Sync,
    // so it cannot borrow `entries`. Move the real commit values into the
    // closure and index them directly — no need to reconstruct the device
    // description from the display label.
    let values: Vec<Option<String>> = entries.into_iter().map(|(_, value)| value).collect();
    let dropdown = cosmic::widget::dropdown(labels, selected, move |idx: usize| -> Message {
        let choice = values.get(idx).cloned().flatten();
        Message::DeviceSelected(choice)
    });

    column![widget::text::caption("DEVICE"), dropdown]
        .spacing(6)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(description: &str) -> DetectedDevice {
        DetectedDevice {
            description: description.to_string(),
            bus: "hid".to_string(),
            address: "/dev/hidraw0".to_string(),
        }
    }

    #[test]
    fn dropdown_items_includes_auto_first() {
        let devices = vec![detected("Corsair Hydro Foo")];
        let items = device_dropdown_items(&devices, None);
        assert!(!items.is_empty());
        assert!(
            items[0].starts_with("Auto"),
            "first item should be Auto, got {:?}",
            items[0],
        );
    }

    #[test]
    fn dropdown_items_appends_disconnected_synthetic_when_saved_missing() {
        let devices = vec![detected("Corsair Hydro Foo")];
        let items = device_dropdown_items(&devices, Some("Corsair iCUE Hbar"));
        assert!(
            items
                .iter()
                .any(|s| s == "Corsair iCUE Hbar (disconnected)"),
            "expected disconnected synthetic in {items:?}",
        );
    }

    #[test]
    fn dropdown_items_omits_synthetic_when_saved_is_connected() {
        let devices = vec![detected("Corsair Hydro Foo")];
        let items = device_dropdown_items(&devices, Some("Corsair Hydro Foo"));
        assert!(
            !items.iter().any(|s| s.ends_with("(disconnected)")),
            "no disconnected row when saved is connected, got {items:?}",
        );
    }

    #[test]
    fn dropdown_items_omits_auto_picked_device_from_explicit_list() {
        // Auto label already names the auto-picked device — listing it
        // again as its own row would be redundant.
        let devices = vec![detected("Corsair Hydro Foo")];
        let items = device_dropdown_items(&devices, None);
        assert_eq!(
            items.len(),
            1,
            "single AIO should yield one item (the Auto row only), got {items:?}",
        );
        assert!(items[0].starts_with("Auto ("));
    }

    #[test]
    fn dropdown_items_lists_non_auto_aios_explicitly() {
        // With multiple AIOs, only the auto-pick is hidden — the others
        // remain available as explicit rows.
        let devices = vec![detected("Corsair Hydro Foo"), detected("Corsair iCUE Hbar")];
        let items = device_dropdown_items(&devices, None);
        assert_eq!(items.len(), 2, "got {items:?}");
        assert!(items[0].starts_with("Auto (Corsair Hydro Foo)"));
        assert_eq!(items[1], "Corsair iCUE Hbar");
    }

    #[test]
    fn dropdown_omits_disconnected_for_connected_non_aio_saved() {
        // m6: a saved device that is connected but not AIO-classified polls
        // fine (effective_match ignores is_aio), so it must NOT be flagged
        // "(disconnected)". The synthetic row is gated on the FULL list.
        let devices = vec![detected("Generic Cooler 9000")];
        let items = device_dropdown_items(&devices, Some("Generic Cooler 9000"));
        assert!(
            !items.iter().any(|s| s.ends_with("(disconnected)")),
            "connected non-AIO saved device must not be marked disconnected, got {items:?}",
        );
    }

    #[test]
    fn dropdown_commits_real_value_not_display_label() {
        // m7: a saved device whose description literally ends in
        // " (disconnected)" must round-trip intact through the entries —
        // the commit value is the real description, not the stripped label.
        let weird = "Corsair Hydro (disconnected)";
        let entries = dropdown_entries(&[], Some(weird));
        // The synthetic row's label gets a suffix appended, but its commit
        // value is the verbatim saved description.
        let synthetic = entries
            .iter()
            .find(|(_, value)| value.as_deref() == Some(weird))
            .expect("saved device should appear as a disconnected entry");
        assert_eq!(synthetic.1.as_deref(), Some(weird));
    }

    #[test]
    fn dropdown_selected_none_maps_to_auto() {
        let entries = dropdown_entries(&[], None);
        assert_eq!(device_dropdown_selected(&[], None, &entries), Some(0));
    }

    #[test]
    fn dropdown_selected_explicit_pick_maps_to_its_row() {
        let devices = vec![detected("Corsair Hydro Foo"), detected("Corsair iCUE Hbar")];
        // Pick the non-auto AIO explicitly; it should resolve to row index 1.
        let saved = Some("Corsair iCUE Hbar");
        let entries = dropdown_entries(&devices, saved);
        assert_eq!(device_dropdown_selected(&devices, saved, &entries), Some(1));
        assert_eq!(entries[1].1.as_deref(), Some("Corsair iCUE Hbar"));
    }

    #[test]
    fn dropdown_selected_explicit_pick_matching_auto_maps_to_auto() {
        let devices = vec![detected("Corsair Hydro Foo")];
        // Saved choice equals what auto would pick — the explicit row is
        // hidden, so selection collapses to Auto (index 0).
        let saved = Some("Corsair Hydro Foo");
        let entries = dropdown_entries(&devices, saved);
        assert_eq!(device_dropdown_selected(&devices, saved, &entries), Some(0));
    }

    #[test]
    fn dropdown_selected_disconnected_saved_maps_to_synthetic_row() {
        let devices = vec![detected("Corsair Hydro Foo")];
        let saved = Some("Corsair iCUE Hbar");
        let entries = dropdown_entries(&devices, saved);
        let idx = device_dropdown_selected(&devices, saved, &entries)
            .expect("disconnected saved choice should resolve to a row");
        assert!(entries[idx].0.ends_with("(disconnected)"));
        assert_eq!(entries[idx].1.as_deref(), Some("Corsair iCUE Hbar"));
    }
}
