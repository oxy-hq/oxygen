//! Row decoding — warehouse rows → `(unit, day ordinal, value)` triples.
//! Pure; re-exported from [`super::panel`].

use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Map, Value};

use oxy_airlayer_compat::engine::experiment::panel_dropping_sparse_units;

use super::panel::{PanelOutcome, PanelRefusal, PanelSpec};

/// A unit's name, exactly as the warehouse rendered it. A string is used
/// as-is — no trim, no case fold; a number renders as JSON prints it, except
/// that an integral float drops its `.0`: a warehouse that types an integer key
/// as a double still names unit `12`, not `12.0`, and that name is what lever
/// automations receive.
pub(super) fn render_key(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => {
                format!("{}", f as i64)
            }
            _ => n.to_string(),
        }),
        _ => None,
    }
}

/// Only the leading `YYYY-MM-DD` is read, so a timestamp passes too.
pub(super) fn parse_day(v: &Value) -> Option<NaiveDate> {
    let s = v.as_str()?;
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

/// The bucket's local start: the date, and the hour when the value carries one
/// (`YYYY-MM-DDTHH…` or `YYYY-MM-DD HH…`); a bare date is its midnight.
pub(super) fn parse_moment(v: &Value) -> Option<NaiveDateTime> {
    let day = parse_day(v)?;
    let s = v.as_str()?;
    let hour = match s.get(10..13) {
        Some(h) if h.starts_with(['T', ' ']) => h[1..].parse::<u32>().ok()?,
        _ => 0,
    };
    day.and_hms_opt(hour, 0, 0)
}

/// A decimal may arrive as a string; a non-finite value is not a value.
pub(super) fn parse_value(v: &Value) -> Option<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    f.is_finite().then_some(f)
}

/// Rows → `(unit, period ordinal, value)`, the ordinal of `spec.grain`. Rows missing a key, a day or a value
/// are skipped and **counted**; if none survive it is a refusal that says how
/// each was lost. Does not itself special-case an empty slice — the caller
/// ([`rows_to_outcome`]) has already returned.
pub fn rows_to_triples(
    rows: &[Map<String, Value>],
    spec: &PanelSpec,
) -> Result<Vec<(String, i64, f64)>, PanelRefusal> {
    let (key_a, day_a, value_a) = (spec.key_alias(), spec.period_alias(), spec.measure_alias());
    let (mut skipped_key, mut skipped_day, mut skipped_value) = (0, 0, 0);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(unit) = row.get(&key_a).and_then(render_key) else {
            skipped_key += 1;
            continue;
        };
        let Some(at) = row.get(&day_a).and_then(parse_moment) else {
            skipped_day += 1;
            continue;
        };
        let Some(value) = row.get(&value_a).and_then(parse_value) else {
            skipped_value += 1;
            continue;
        };
        let when = at.date();
        if !spec.grain.is_start(when) {
            return Err(PanelRefusal::PeriodNotAligned {
                measure: spec.measure.clone(),
                grain: spec.grain,
                date: when,
                weekday: when.format("%A").to_string(),
            });
        }
        out.push((unit, spec.grain.ordinal_at(at), value));
    }
    if out.is_empty() {
        return Err(PanelRefusal::NoUsableRows {
            measure: spec.measure.clone(),
            entity: spec.entity.clone(),
            rows: rows.len(),
            skipped_key,
            skipped_day,
            skipped_value,
        });
    }
    Ok(out)
}

/// [`super::panel::rows_to_outcome`] with a unit floor: a unit holding fewer than
/// `floor` of the panel's days is dropped and listed, instead of every unit
/// losing the days it lacks.
pub fn rows_to_thinned(
    rows: &[Map<String, Value>],
    spec: &PanelSpec,
    floor: f64,
) -> Result<PanelOutcome, PanelRefusal> {
    if rows.is_empty() {
        return Ok(PanelOutcome::NoHistory {
            measure: spec.measure.clone(),
            entity: spec.entity.clone(),
        });
    }
    let (panel, dropped) = panel_dropping_sparse_units(rows_to_triples(rows, spec)?, floor);
    Ok(PanelOutcome::Thinned { panel, dropped })
}
