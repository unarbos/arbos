//! Chart bubbles: the structured chart an agent posts with `post_chart`, validated against
//! the caps in [`SCHEMA_JSON`]; the resolved [`ChartModel`] (domains, ticks, formatted values,
//! readout columns) the iPhone's Swift Charts draws from; the pixel-level [`PlotLayout`] the Mac
//! canvas and the PNG fallback paint; the hit test behind hover / touch-and-hold; and the table
//! export behind "Copy data" and the text-only fallback. Shared by agentd (validate, render the
//! PNG), the Mac app and, through `arbos-ffi`, the iPhone — so every surface agrees on the numbers.
//!
//! No I/O, no drawing primitives: geometry only.

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub const SPEC_VERSION: u32 = 1;
pub const MAX_SERIES: usize = 8;
pub const MAX_POINTS_PER_SERIES: usize = 500;
pub const MAX_POINTS_TOTAL: usize = 2000;
pub const MAX_CATEGORIES: usize = 100;
pub const MAX_PIE_SLICES: usize = 12;
pub const MAX_ANNOTATIONS: usize = 12;
/// The serialised `chart` object (compact JSON) must fit in the doc part.
pub const MAX_SPEC_BYTES: usize = 32 * 1024;
pub const MAX_TITLE: usize = 80;
pub const MAX_SUBTITLE: usize = 120;
pub const MAX_NAME: usize = 40;
pub const MAX_UNIT: usize = 12;
pub const MAX_CAPTION: usize = 300;

/// JSON schema of the `chart` argument of `post_chart` (advertised by the MCP server; the same
/// caps [`Chart::parse`] enforces).
pub const SCHEMA_JSON: &str = include_str!("chart.schema.json");

/// What the tool answers when the spec is over a cap.
pub const TOO_BIG_HINT: &str =
    "downsample (fewer points: a bucket per day, every nth sample) or post the data as a CSV with deliver_file";

// ------------------------------------------------------------------ spec

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Line,
    Area,
    Bar,
    Scatter,
    Pie,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Line => "line",
            Kind::Area => "area",
            Kind::Bar => "bar",
            Kind::Scatter => "scatter",
            Kind::Pie => "pie",
        }
    }

    /// Series carry `points` (x/y pairs) rather than `values` per category.
    pub fn is_xy(self) -> bool {
        matches!(self, Kind::Line | Kind::Area | Kind::Scatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    #[default]
    Linear,
    Log,
    Time,
    Category,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Auto,
    Integer,
    Number,
    Percent,
    Compact,
    Currency,
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AxisSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Legend {
    #[default]
    Auto,
    Top,
    Bottom,
    None,
}

/// An x value as the agent sent it: a number, or a text (an ISO date on a time axis, a label on
/// a category axis).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum XValue {
    Num(f64),
    Text(String),
}

/// `[x, y]`; `y` may be `null` (a gap).
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub x: XValue,
    pub y: Option<f64>,
}

impl Serialize for Point {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(2))?;
        seq.serialize_element(&self.x)?;
        seq.serialize_element(&self.y)?;
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Point {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<Value> = Vec::deserialize(d)?;
        if v.len() != 2 {
            return Err(serde::de::Error::custom("a point is [x, y]"));
        }
        let x = match &v[0] {
            Value::Number(n) => XValue::Num(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => XValue::Text(s.clone()),
            _ => return Err(serde::de::Error::custom("x must be a number or a string")),
        };
        let y = match &v[1] {
            Value::Null => None,
            Value::Number(n) => Some(n.as_f64().unwrap_or(f64::NAN)),
            _ => return Err(serde::de::Error::custom("y must be a number or null")),
        };
        Ok(Point { x, y })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Option<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub style: LineStyle,
    #[serde(default)]
    pub axis: AxisSide,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Axis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    Vline,
    Hline,
    Point,
    Band,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    #[serde(rename = "type")]
    pub kind: AnnotationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<XValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<XValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<XValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The validated `chart` argument, stored as sent in the `chart` doc part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chart {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub series: Vec<Series>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default_axis")]
    pub x: Axis,
    #[serde(default, skip_serializing_if = "is_default_axis")]
    pub y: Axis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y2: Option<Axis>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stacked: bool,
    #[serde(default)]
    pub legend: Legend,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

fn is_default_axis(a: &Axis) -> bool {
    *a == Axis::default()
}

/// Parse an ISO date / date-time (`2026-10-26`, `2026-10-26T14:00:00Z`, `2026-10-26 14:00`) to
/// epoch seconds (UTC).
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp() as f64);
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, f) {
            return Some(dt.and_utc().timestamp() as f64);
        }
    }
    for f in ["%Y-%m-%d", "%Y-%m"] {
        let padded = if f == "%Y-%m" {
            format!("{s}-01")
        } else {
            s.to_owned()
        };
        if let Ok(d) = NaiveDate::parse_from_str(&padded, "%Y-%m-%d") {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64);
        }
    }
    None
}

fn check_len(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    Ok(())
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

impl Chart {
    /// Parse and validate the `chart` argument: shape (unknown fields are errors, so a typo never
    /// silently drops an axis), the caps, and the per-kind rules. The error text is for the agent.
    pub fn parse(v: &Value) -> Result<Chart, String> {
        let bytes = serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX);
        if bytes > MAX_SPEC_BYTES {
            return Err(format!(
                "chart is {bytes} bytes; the limit is {MAX_SPEC_BYTES} — {TOO_BIG_HINT}"
            ));
        }
        let chart: Chart = serde_json::from_value(v.clone()).map_err(|e| format!("chart: {e}"))?;
        chart.validate()?;
        Ok(chart)
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(t) = &self.title {
            check_len("title", t, MAX_TITLE)?;
        }
        if let Some(t) = &self.subtitle {
            check_len("subtitle", t, MAX_SUBTITLE)?;
        }
        if self.series.is_empty() {
            return Err("chart needs at least one series".into());
        }
        if self.series.len() > MAX_SERIES {
            return Err(format!(
                "chart has {} series; the limit is {MAX_SERIES} — {TOO_BIG_HINT}",
                self.series.len()
            ));
        }
        for axis in [Some(&self.x), Some(&self.y), self.y2.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(l) = &axis.label {
                check_len("axis label", l, MAX_NAME)?;
            }
            if let Some(u) = &axis.unit {
                check_len("axis unit", u, MAX_UNIT)?;
            }
            if let (Some(lo), Some(hi)) = (axis.min, axis.max) {
                if lo >= hi {
                    return Err("axis min must be below max".into());
                }
            }
            for v in [axis.min, axis.max].into_iter().flatten() {
                if !v.is_finite() {
                    return Err("axis min/max must be finite numbers".into());
                }
            }
        }
        if self.stacked && !matches!(self.kind, Kind::Bar | Kind::Area) {
            return Err("stacked applies to bar and area charts only".into());
        }
        if self.annotations.len() > MAX_ANNOTATIONS {
            return Err(format!(
                "chart has {} annotations; the limit is {MAX_ANNOTATIONS}",
                self.annotations.len()
            ));
        }
        for a in &self.annotations {
            if let Some(l) = &a.label {
                check_len("annotation label", l, MAX_NAME)?;
            }
            let ok = match a.kind {
                AnnotationKind::Vline => a.x.is_some(),
                AnnotationKind::Hline => a.y.is_some(),
                AnnotationKind::Point => a.x.is_some() && a.y.is_some(),
                AnnotationKind::Band => a.from.is_some() && a.to.is_some(),
            };
            if !ok {
                return Err(format!(
                    "annotation {:?} needs {}",
                    a.kind,
                    match a.kind {
                        AnnotationKind::Vline => "x",
                        AnnotationKind::Hline => "y",
                        AnnotationKind::Point => "x and y",
                        AnnotationKind::Band => "from and to",
                    }
                ));
            }
            if self.kind == Kind::Pie {
                return Err("pie charts take no annotations".into());
            }
        }
        for s in &self.series {
            if s.name.trim().is_empty() {
                return Err("every series needs a name".into());
            }
            check_len("series name", &s.name, MAX_NAME)?;
            if let Some(c) = &s.color {
                if !valid_color(c) {
                    return Err(format!("series {:?}: color must be #RRGGBB", s.name));
                }
            }
            if s.axis == AxisSide::Right && !self.kind.is_xy() {
                return Err("a right axis applies to line, area and scatter charts only".into());
            }
        }
        if self.kind.is_xy() {
            self.validate_xy()
        } else {
            self.validate_categorical()
        }
    }

    fn validate_xy(&self) -> Result<(), String> {
        if !self.categories.is_empty() {
            return Err(format!(
                "{} charts take series.points, not categories",
                self.kind.as_str()
            ));
        }
        let mut total = 0usize;
        let scale = self.x.scale.unwrap_or_default();
        let mut any_text = false;
        let mut any_num = false;
        for s in &self.series {
            if !s.values.is_empty() {
                return Err(format!(
                    "series {:?}: {} charts take points ([x, y] pairs), not values",
                    s.name,
                    self.kind.as_str()
                ));
            }
            if s.points.is_empty() {
                return Err(format!("series {:?} has no points", s.name));
            }
            if s.points.len() > MAX_POINTS_PER_SERIES {
                return Err(format!(
                    "series {:?} has {} points; the limit is {MAX_POINTS_PER_SERIES} per series — {TOO_BIG_HINT}",
                    s.name,
                    s.points.len()
                ));
            }
            total += s.points.len();
            for p in &s.points {
                match &p.x {
                    XValue::Num(n) if n.is_finite() => any_num = true,
                    XValue::Num(_) => return Err(format!("series {:?}: x must be finite", s.name)),
                    XValue::Text(t) => {
                        any_text = true;
                        if scale == Scale::Time && parse_time(t).is_none() {
                            return Err(format!(
                                "series {:?}: x {t:?} is not an ISO date (yyyy-mm-dd or yyyy-mm-ddThh:mm:ssZ)",
                                s.name
                            ));
                        }
                    }
                }
                if let Some(y) = p.y {
                    if !y.is_finite() {
                        return Err(format!("series {:?}: y must be finite or null", s.name));
                    }
                    let log = match s.axis {
                        AxisSide::Left => self.y.scale == Some(Scale::Log),
                        AxisSide::Right => {
                            self.y2.as_ref().and_then(|a| a.scale) == Some(Scale::Log)
                        }
                    };
                    if log && y <= 0.0 {
                        return Err(format!(
                            "series {:?}: a log axis needs values above 0",
                            s.name
                        ));
                    }
                }
            }
        }
        if total > MAX_POINTS_TOTAL {
            return Err(format!(
                "chart has {total} points; the limit is {MAX_POINTS_TOTAL} in total — {TOO_BIG_HINT}"
            ));
        }
        match scale {
            Scale::Time => {
                if any_num && any_text {
                    return Err("a time axis takes either ISO date strings or numbers (epoch seconds), not both".into());
                }
            }
            Scale::Category => {
                if any_num && any_text {
                    return Err("a category axis takes the same kind of x on every point".into());
                }
            }
            Scale::Linear | Scale::Log => {
                if any_text {
                    // Text x without a scale: dates are a time axis, anything else a category axis.
                    let all_dates =
                        self.series
                            .iter()
                            .flat_map(|s| &s.points)
                            .all(|p| match &p.x {
                                XValue::Text(t) => parse_time(t).is_some(),
                                XValue::Num(_) => false,
                            });
                    if self.x.scale.is_some() || (any_num && !all_dates) {
                        return Err("x values are text: set x.scale to \"time\" (ISO dates) or \"category\" (labels)".into());
                    }
                }
                if scale == Scale::Log {
                    for s in &self.series {
                        for p in &s.points {
                            if let XValue::Num(n) = p.x {
                                if n <= 0.0 {
                                    return Err("a log x axis needs values above 0".into());
                                }
                            }
                        }
                    }
                }
            }
        }
        if self.y.scale == Some(Scale::Time) || self.y.scale == Some(Scale::Category) {
            return Err("the value axis is linear or log".into());
        }
        Ok(())
    }

    fn validate_categorical(&self) -> Result<(), String> {
        if self.categories.is_empty() {
            return Err(format!(
                "{} charts need categories (the labels) and one values list per series",
                self.kind.as_str()
            ));
        }
        if self.categories.len() > MAX_CATEGORIES {
            return Err(format!(
                "chart has {} categories; the limit is {MAX_CATEGORIES} — {TOO_BIG_HINT}",
                self.categories.len()
            ));
        }
        for c in &self.categories {
            check_len("category", c, MAX_NAME)?;
        }
        if self.kind == Kind::Pie {
            if self.series.len() != 1 {
                return Err("a pie chart takes exactly one series".into());
            }
            if self.categories.len() > MAX_PIE_SLICES {
                return Err(format!(
                    "a pie chart has at most {MAX_PIE_SLICES} slices; group the rest as \"Other\""
                ));
            }
        }
        for s in &self.series {
            if !s.points.is_empty() {
                return Err(format!(
                    "series {:?}: {} charts take values (one per category), not points",
                    s.name,
                    self.kind.as_str()
                ));
            }
            if s.values.len() != self.categories.len() {
                return Err(format!(
                    "series {:?} has {} values for {} categories",
                    s.name,
                    s.values.len(),
                    self.categories.len()
                ));
            }
            for v in s.values.iter().flatten() {
                if !v.is_finite() {
                    return Err(format!(
                        "series {:?}: values must be finite or null",
                        s.name
                    ));
                }
                if self.kind == Kind::Pie && *v < 0.0 {
                    return Err("pie slices cannot be negative".into());
                }
                if self.y.scale == Some(Scale::Log) && *v <= 0.0 {
                    return Err(format!(
                        "series {:?}: a log axis needs values above 0",
                        s.name
                    ));
                }
            }
        }
        if self.kind == Kind::Pie {
            let sum: f64 = self.series[0].values.iter().flatten().sum();
            if sum <= 0.0 {
                return Err("pie slices sum to zero".into());
            }
        }
        Ok(())
    }

    /// The x value of a point as a number on the resolved x scale.
    fn x_num(&self, x: &XValue, scale: Scale) -> Option<f64> {
        match (x, scale) {
            (XValue::Num(n), _) => Some(*n),
            (XValue::Text(t), Scale::Time) => parse_time(t),
            (XValue::Text(t), Scale::Category) => self
                .x_categories()
                .iter()
                .position(|c| c == t)
                .map(|i| i as f64),
            (XValue::Text(t), _) => parse_time(t),
        }
    }

    /// Distinct text x labels in first-seen order (category x axis on an xy chart).
    fn x_categories(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.series {
            for p in &s.points {
                if let XValue::Text(t) = &p.x {
                    if !out.contains(t) {
                        out.push(t.clone());
                    }
                }
            }
        }
        out
    }

    /// The resolved x scale: as given, else time for date strings, category for other strings.
    pub fn x_scale(&self) -> Scale {
        if let Some(s) = self.x.scale {
            return s;
        }
        if !self.kind.is_xy() {
            return Scale::Category;
        }
        let texts: Vec<&str> = self
            .series
            .iter()
            .flat_map(|s| &s.points)
            .filter_map(|p| match &p.x {
                XValue::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        if texts.is_empty() {
            Scale::Linear
        } else if texts.iter().all(|t| parse_time(t).is_some()) {
            Scale::Time
        } else {
            Scale::Category
        }
    }
}

// ------------------------------------------------------------------ number formatting

fn trim_float(v: f64, max_decimals: usize) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    let mut s = format!("{v:.*}", max_decimals);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

fn with_thousands(int_part: &str) -> String {
    let (sign, digits) = match int_part.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", int_part),
    };
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}")
}

/// Decimals that distinguish values `step` apart (ticks), at most 3.
fn decimals_for_step(step: f64) -> usize {
    if step <= 0.0 || !step.is_finite() {
        return 0;
    }
    let mut d = 0usize;
    let mut s = step;
    while d < 3 && (s - s.round()).abs() > 1e-6 {
        s *= 10.0;
        d += 1;
    }
    d
}

fn compact(v: f64) -> String {
    let a = v.abs();
    let (div, suffix) = if a >= 1e12 {
        (1e12, "T")
    } else if a >= 1e9 {
        (1e9, "B")
    } else if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else {
        return trim_float(v, 2);
    };
    let scaled = v / div;
    let dec = if scaled.abs() >= 100.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    format!("{}{suffix}", trim_float(scaled, dec))
}

fn duration(v: f64) -> String {
    let neg = v < 0.0;
    let s = v.abs();
    let body = if s < 60.0 {
        format!("{} s", trim_float(s, 1))
    } else if s < 3600.0 {
        let m = (s / 60.0).floor();
        let sec = (s - m * 60.0).round();
        if sec == 0.0 {
            format!("{m:.0}m")
        } else {
            format!("{m:.0}m {sec:.0}s")
        }
    } else if s < 86400.0 {
        let h = (s / 3600.0).floor();
        let m = ((s - h * 3600.0) / 60.0).round();
        if m == 0.0 {
            format!("{h:.0}h")
        } else {
            format!("{h:.0}h {m:02.0}m")
        }
    } else {
        let d = (s / 86400.0).floor();
        let h = ((s - d * 86400.0) / 3600.0).round();
        if h == 0.0 {
            format!("{d:.0}d")
        } else {
            format!("{d:.0}d {h:.0}h")
        }
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Format a value for an axis: `format` decides the number shape, `unit` the suffix (`%` glued,
/// anything else after a space; a currency symbol goes in front). `decimals` is the tick
/// precision when formatting ticks (`None` for a data value: up to 2 significant decimals).
pub fn format_value(v: f64, format: Format, unit: &str, decimals: Option<usize>) -> String {
    let dec = decimals.unwrap_or(if v.abs() >= 100.0 { 1 } else { 2 });
    let number = match format {
        Format::Integer => with_thousands(&format!("{:.0}", v.round())),
        Format::Number | Format::Auto => {
            let s = trim_float(v, dec);
            if v.abs() >= 10_000.0 {
                match s.split_once('.') {
                    Some((i, f)) => format!("{}.{f}", with_thousands(i)),
                    None => with_thousands(&s),
                }
            } else {
                s
            }
        }
        Format::Percent => return format!("{}%", trim_float(v, dec.min(1))),
        Format::Compact => compact(v),
        Format::Currency => {
            let sym = if unit.is_empty() || unit.chars().all(|c| c.is_ascii_alphabetic()) {
                "$"
            } else {
                unit
            };
            let s = if v.abs() >= 1000.0 || decimals.is_some() {
                with_thousands(&format!("{:.0}", v.round()))
            } else {
                trim_float(v, 2)
            };
            let (sign, body) = match s.strip_prefix('-') {
                Some(b) => ("-", b),
                None => ("", s.as_str()),
            };
            let out = format!("{sign}{sym}{body}");
            return if unit.chars().all(|c| c.is_ascii_alphabetic()) && !unit.is_empty() {
                format!("{out} {unit}")
            } else {
                out
            };
        }
        Format::Duration => return duration(v),
    };
    match unit {
        "" => number,
        "%" => format!("{number}%"),
        u => format!("{number} {u}"),
    }
}

fn format_time(t: f64, span: f64) -> String {
    let Some(dt) = Utc.timestamp_opt(t.round() as i64, 0).single() else {
        return trim_float(t, 0);
    };
    if span <= 2.0 * 86400.0 {
        format!("{:02}:{:02}", dt.hour(), dt.minute())
    } else if span <= 400.0 * 86400.0 {
        let m = month_abbr(dt.month());
        format!("{} {m}", dt.day())
    } else {
        format!("{} {}", month_abbr(dt.month()), dt.year())
    }
}

fn format_time_full(t: f64) -> String {
    let Some(dt) = Utc.timestamp_opt(t.round() as i64, 0).single() else {
        return trim_float(t, 0);
    };
    if dt.hour() == 0 && dt.minute() == 0 && dt.second() == 0 {
        format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    }
}

fn month_abbr(m: u32) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m.clamp(1, 12) - 1) as usize]
}

// ------------------------------------------------------------------ ticks

/// A "nice" step (1 · 2 · 2.5 · 5 × 10ⁿ) splitting `lo..hi` into about `n` intervals.
pub fn nice_step(lo: f64, hi: f64, n: usize) -> f64 {
    let span = (hi - lo).abs().max(f64::MIN_POSITIVE);
    let raw = span / n.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let r = raw / mag;
    let m = if r <= 1.0 {
        1.0
    } else if r <= 2.0 {
        2.0
    } else if r <= 2.5 {
        2.5
    } else if r <= 5.0 {
        5.0
    } else {
        10.0
    };
    m * mag
}

/// Tick values at multiples of `step` inside `lo..=hi`.
pub fn ticks_in(lo: f64, hi: f64, step: f64) -> Vec<f64> {
    if step <= 0.0 || !step.is_finite() || lo >= hi {
        return vec![lo, hi];
    }
    let mut out = Vec::new();
    let mut k = (lo / step - 1e-9).ceil();
    while k * step <= hi + step * 1e-9 && out.len() < 50 {
        let v = k * step;
        out.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        k += 1.0;
    }
    out
}

fn time_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let span = hi - lo;
    let units: [f64; 12] = [
        60.0,
        300.0,
        900.0,
        1800.0,
        3600.0,
        3.0 * 3600.0,
        6.0 * 3600.0,
        12.0 * 3600.0,
        86400.0,
        7.0 * 86400.0,
        30.0 * 86400.0,
        365.0 * 86400.0,
    ];
    let raw = span / n.max(1) as f64;
    let step = units
        .iter()
        .copied()
        .find(|u| *u >= raw)
        .unwrap_or(365.0 * 86400.0);
    if step >= 30.0 * 86400.0 {
        // Month / year boundaries.
        let Some(start) = Utc.timestamp_opt(lo as i64, 0).single() else {
            return ticks_in(lo, hi, step);
        };
        let months = if step >= 365.0 * 86400.0 { 12 } else { 1 };
        let mut y = start.year();
        let mut m = start.month();
        if start.day() != 1 || start.hour() != 0 {
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
        }
        if months == 12 && m != 1 {
            m = 1;
            y += 1;
        }
        let mut out = Vec::new();
        while out.len() < 50 {
            let Some(t) = NaiveDate::from_ymd_opt(y, m, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc().timestamp() as f64)
            else {
                break;
            };
            if t > hi + 1.0 {
                break;
            }
            out.push(t);
            m += months;
            while m > 12 {
                m -= 12;
                y += 1;
            }
        }
        if out.len() < 2 {
            return vec![lo, hi];
        }
        out
    } else {
        ticks_in(lo, hi, step)
    }
}

// ------------------------------------------------------------------ resolved model

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    pub value: f64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisModel {
    pub label: Option<String>,
    pub unit: String,
    pub scale: Scale,
    pub format: Format,
    pub min: f64,
    pub max: f64,
    pub ticks: Vec<Tick>,
    /// Category axes: the labels, index = position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

impl AxisModel {
    /// Fraction 0..1 of `v` along the axis (log-aware). Values outside clamp.
    pub fn frac(&self, v: f64) -> f64 {
        let f = if self.scale == Scale::Log {
            let lo = self.min.max(f64::MIN_POSITIVE).log10();
            let hi = self.max.max(f64::MIN_POSITIVE).log10();
            (v.max(f64::MIN_POSITIVE).log10() - lo) / (hi - lo)
        } else {
            (v - self.min) / (self.max - self.min)
        };
        if f.is_finite() {
            f.clamp(-0.5, 1.5)
        } else {
            0.0
        }
    }

    /// A data value on this axis, formatted with its unit.
    pub fn text(&self, v: f64) -> String {
        if self.scale == Scale::Time {
            return format_time_full(v);
        }
        if self.scale == Scale::Category {
            return self
                .categories
                .get(v.round() as usize)
                .cloned()
                .unwrap_or_else(|| trim_float(v, 0));
        }
        format_value(v, self.format, &self.unit, None)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointModel {
    pub x: f64,
    pub y: Option<f64>,
    /// Stack base (0 when not stacked).
    pub y0: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesModel {
    pub name: String,
    /// Index into the theme palette (series order).
    pub color_index: usize,
    pub color: Option<String>,
    pub style: LineStyle,
    pub axis: AxisSide,
    pub points: Vec<PointModel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnnotationModel {
    pub kind: AnnotationKind,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnEntry {
    pub series: usize,
    pub value: f64,
    pub text: String,
}

/// One readout column: every series' value at one x (or category), what hover / touch shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub x: f64,
    pub label: String,
    pub entries: Vec<ColumnEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartModel {
    pub kind: Kind,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub series: Vec<SeriesModel>,
    pub categories: Vec<String>,
    pub x: AxisModel,
    pub y: AxisModel,
    pub y2: Option<AxisModel>,
    pub stacked: bool,
    pub show_legend: bool,
    pub legend_bottom: bool,
    pub annotations: Vec<AnnotationModel>,
    pub columns: Vec<Column>,
    /// Pie: share of each slice, 0..1, in category order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<f64>,
}

fn axis_model(
    axis: &Axis,
    scale: Scale,
    data_lo: f64,
    data_hi: f64,
    zero_based: bool,
    n: usize,
) -> AxisModel {
    let unit = axis.unit.clone().unwrap_or_default();
    let format = axis.format.unwrap_or(if unit == "%" {
        Format::Percent
    } else {
        Format::Auto
    });
    let (mut lo, mut hi) = (data_lo, data_hi);
    if zero_based {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if lo >= hi {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        lo -= pad;
        hi += pad;
    }
    let (min, max, ticks) = match scale {
        Scale::Log => {
            let lo = axis.min.unwrap_or(lo).max(f64::MIN_POSITIVE);
            let hi = axis.max.unwrap_or(hi).max(lo * 10.0);
            let a = lo.log10().floor() as i32;
            let b = hi.log10().ceil() as i32;
            let min = axis.min.unwrap_or(10f64.powi(a));
            let max = axis.max.unwrap_or(10f64.powi(b));
            let ticks = (a..=b)
                .map(|e| 10f64.powi(e))
                .filter(|v| *v >= min * 0.999 && *v <= max * 1.001)
                .map(|v| Tick {
                    value: v,
                    label: format_value(v, format, &unit, Some(0)),
                })
                .collect();
            (min, max, ticks)
        }
        Scale::Time => {
            let min = axis.min.unwrap_or(lo);
            let max = axis.max.unwrap_or(hi);
            let span = max - min;
            let ticks = time_ticks(min, max, n)
                .into_iter()
                .map(|v| Tick {
                    value: v,
                    label: format_time(v, span),
                })
                .collect();
            (min, max, ticks)
        }
        Scale::Category => {
            // Positions 0..len; ticks at every category (the layout thins them).
            (lo, hi, Vec::new())
        }
        Scale::Linear => {
            let step = nice_step(axis.min.unwrap_or(lo), axis.max.unwrap_or(hi), n);
            let min = axis.min.unwrap_or((lo / step).floor() * step);
            let max = axis.max.unwrap_or((hi / step).ceil() * step);
            let step = if axis.min.is_some() || axis.max.is_some() {
                nice_step(min, max, n)
            } else {
                step
            };
            let dec = decimals_for_step(step);
            let ticks = ticks_in(min, max, step)
                .into_iter()
                .map(|v| Tick {
                    value: v,
                    label: format_value(v, format, &unit, Some(dec)),
                })
                .collect();
            (min, max, ticks)
        }
    };
    AxisModel {
        label: axis.label.clone(),
        unit,
        scale,
        format,
        min,
        max,
        ticks,
        categories: Vec::new(),
    }
}

impl Chart {
    /// Resolve scales, domains, ticks, stacking and readout columns. Pure data: the iPhone draws
    /// Swift Charts from it; [`ChartModel::layout`] turns it into pixels for the Mac and the PNG.
    pub fn model(&self) -> ChartModel {
        let x_scale = self.x_scale();
        let x_cats = if self.kind.is_xy() {
            if x_scale == Scale::Category {
                self.x_categories()
            } else {
                Vec::new()
            }
        } else {
            self.categories.clone()
        };
        // Series points on numeric x.
        let mut series: Vec<SeriesModel> = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let points = if self.kind.is_xy() {
                    let mut pts: Vec<PointModel> = s
                        .points
                        .iter()
                        .filter_map(|p| {
                            self.x_num(&p.x, x_scale)
                                .map(|x| PointModel { x, y: p.y, y0: 0.0 })
                        })
                        .collect();
                    if self.kind != Kind::Scatter {
                        pts.sort_by(|a, b| {
                            a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                    pts
                } else {
                    s.values
                        .iter()
                        .enumerate()
                        .map(|(ci, v)| PointModel {
                            x: ci as f64,
                            y: *v,
                            y0: 0.0,
                        })
                        .collect()
                };
                SeriesModel {
                    name: s.name.clone(),
                    color_index: i,
                    color: s.color.clone(),
                    style: s.style,
                    axis: s.axis,
                    points,
                }
            })
            .collect();
        // Stacking: each series sits on the sum of the earlier ones at the same x.
        if self.stacked {
            let mut tops: Vec<(f64, f64)> = Vec::new();
            for s in &mut series {
                for p in &mut s.points {
                    let Some(y) = p.y else { continue };
                    let base = tops
                        .iter()
                        .find(|(x, _)| (*x - p.x).abs() < 1e-9)
                        .map(|(_, t)| *t)
                        .unwrap_or(0.0);
                    p.y0 = base;
                    p.y = Some(base + y);
                    match tops.iter_mut().find(|(x, _)| (*x - p.x).abs() < 1e-9) {
                        Some(t) => t.1 = base + y,
                        None => tops.push((p.x, base + y)),
                    }
                }
            }
        }
        let xs: Vec<f64> = series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.x))
            .collect();
        let (xlo, xhi) = min_max(&xs).unwrap_or((0.0, 1.0));
        let ys = |side: AxisSide| -> Vec<f64> {
            series
                .iter()
                .filter(|s| s.axis == side)
                .flat_map(|s| s.points.iter().filter_map(|p| p.y))
                .chain(if side == AxisSide::Left {
                    self.annotations
                        .iter()
                        .filter_map(|a| a.y)
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                })
                .collect()
        };
        let zero_based = matches!(self.kind, Kind::Bar | Kind::Area);
        let (ylo, yhi) = min_max(&ys(AxisSide::Left)).unwrap_or((0.0, 1.0));
        let mut x = if self.kind.is_xy() {
            axis_model(&self.x, x_scale, xlo, xhi, false, 5)
        } else {
            AxisModel {
                label: self.x.label.clone(),
                unit: String::new(),
                scale: Scale::Category,
                format: Format::Auto,
                min: -0.5,
                max: self.categories.len() as f64 - 0.5,
                ticks: Vec::new(),
                categories: Vec::new(),
            }
        };
        if x_scale == Scale::Category {
            x.categories = x_cats.clone();
            if self.kind.is_xy() {
                x.min = -0.5;
                x.max = x_cats.len() as f64 - 0.5;
            }
            x.ticks = x_cats
                .iter()
                .enumerate()
                .map(|(i, c)| Tick {
                    value: i as f64,
                    label: c.clone(),
                })
                .collect();
        } else if x_scale == Scale::Linear {
            // Integer-valued x (run numbers) never gets fractional ticks.
            let all_int = xs.iter().all(|v| (v - v.round()).abs() < 1e-9);
            if all_int {
                x.ticks.retain(|t| (t.value - t.value.round()).abs() < 1e-9);
                for t in &mut x.ticks {
                    t.label = trim_float(t.value, 0);
                }
                if self.x.min.is_none() {
                    x.min = xlo;
                }
                if self.x.max.is_none() {
                    x.max = xhi;
                }
                x.ticks
                    .retain(|t| t.value >= x.min - 1e-9 && t.value <= x.max + 1e-9);
                if !x.ticks.iter().any(|t| (t.value - xlo).abs() < 1e-9) {
                    x.ticks.insert(
                        0,
                        Tick {
                            value: xlo,
                            label: trim_float(xlo, 0),
                        },
                    );
                }
            }
        }
        let y = if self.kind == Kind::Pie {
            axis_model(&self.y, Scale::Linear, 0.0, 1.0, true, 4)
        } else {
            axis_model(
                &self.y,
                if self.y.scale == Some(Scale::Log) {
                    Scale::Log
                } else {
                    Scale::Linear
                },
                ylo,
                yhi,
                zero_based,
                4,
            )
        };
        let y2 = if series.iter().any(|s| s.axis == AxisSide::Right) {
            let (lo, hi) = min_max(&ys(AxisSide::Right)).unwrap_or((0.0, 1.0));
            let a = self.y2.clone().unwrap_or_default();
            Some(axis_model(
                &a,
                if a.scale == Some(Scale::Log) {
                    Scale::Log
                } else {
                    Scale::Linear
                },
                lo,
                hi,
                zero_based,
                4,
            ))
        } else {
            None
        };
        let annotations = self
            .annotations
            .iter()
            .map(|a| AnnotationModel {
                kind: a.kind,
                x: a.x.as_ref().and_then(|v| self.x_num(v, x_scale)),
                y: a.y,
                from: a.from.as_ref().and_then(|v| self.x_num(v, x_scale)),
                to: a.to.as_ref().and_then(|v| self.x_num(v, x_scale)),
                label: a.label.clone(),
            })
            .collect();
        let show_legend = match self.legend {
            Legend::None => false,
            Legend::Auto => self.series.len() > 1 || self.kind == Kind::Pie,
            Legend::Top | Legend::Bottom => true,
        };
        let shares = if self.kind == Kind::Pie {
            let vals: Vec<f64> = series[0]
                .points
                .iter()
                .map(|p| p.y.unwrap_or(0.0))
                .collect();
            let sum: f64 = vals.iter().sum();
            vals.iter()
                .map(|v| if sum > 0.0 { v / sum } else { 0.0 })
                .collect()
        } else {
            Vec::new()
        };
        let mut model = ChartModel {
            kind: self.kind,
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            series,
            categories: self.categories.clone(),
            x,
            y,
            y2,
            stacked: self.stacked,
            show_legend,
            legend_bottom: self.legend == Legend::Bottom,
            annotations,
            columns: Vec::new(),
            shares,
        };
        model.columns = model.build_columns(self);
        model
    }
}

fn min_max(v: &[f64]) -> Option<(f64, f64)> {
    let mut it = v.iter().copied().filter(|x| x.is_finite());
    let first = it.next()?;
    Some(it.fold((first, first), |(lo, hi), x| (lo.min(x), hi.max(x))))
}

impl ChartModel {
    fn build_columns(&self, chart: &Chart) -> Vec<Column> {
        let mut xs: Vec<f64> = Vec::new();
        for s in &self.series {
            for p in &s.points {
                if !xs.iter().any(|x| (*x - p.x).abs() < 1e-9) {
                    xs.push(p.x);
                }
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        xs.iter()
            .map(|&x| {
                let label = if self.kind.is_xy() {
                    match self.x.scale {
                        Scale::Category => self.x.text(x),
                        Scale::Time => format_time_full(x),
                        _ => {
                            let base = trim_float(x, 2);
                            match &self.x.label {
                                Some(l) if !self.x.unit.is_empty() => {
                                    format!("{l} {base} {}", self.x.unit)
                                }
                                Some(l) => format!("{l} {base}"),
                                None if !self.x.unit.is_empty() => {
                                    format!("{base} {}", self.x.unit)
                                }
                                None => base,
                            }
                        }
                    }
                } else {
                    self.categories.get(x as usize).cloned().unwrap_or_default()
                };
                let entries = self
                    .series
                    .iter()
                    .enumerate()
                    .filter_map(|(si, s)| {
                        let p = s.points.iter().find(|p| (p.x - x).abs() < 1e-9)?;
                        let raw = p.y? - p.y0; // the series' own value, not the stacked top
                        let axis = match s.axis {
                            AxisSide::Left => &self.y,
                            AxisSide::Right => self.y2.as_ref().unwrap_or(&self.y),
                        };
                        let mut text = axis.text(raw);
                        if chart.kind == Kind::Pie {
                            let share = self.shares.get(p.x as usize).copied().unwrap_or(0.0);
                            text = format!("{text} · {}%", trim_float(share * 100.0, 1));
                        }
                        Some(ColumnEntry {
                            series: si,
                            value: raw,
                            text,
                        })
                    })
                    .collect();
                Column { x, label, entries }
            })
            .collect()
    }

    /// Nearest readout column to a data-space x (the iPhone's selection), within half a step.
    pub fn column_near(&self, x: f64) -> Option<usize> {
        if self.columns.is_empty() {
            return None;
        }
        let (i, c) = self.columns.iter().enumerate().min_by(|(_, a), (_, b)| {
            (a.x - x)
                .abs()
                .partial_cmp(&(b.x - x).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let step = if self.columns.len() > 1 {
            (self.columns[1].x - self.columns[0].x).abs().max(1e-9)
        } else {
            f64::INFINITY
        };
        ((c.x - x).abs() <= step).then_some(i)
    }

    /// Colour slot for a series: its own colour, or the palette index.
    pub fn series_color<'a>(&self, i: usize, palette: &'a [&'a str]) -> String {
        match self.series.get(i).and_then(|s| s.color.as_deref()) {
            Some(c) => c.to_owned(),
            None => palette
                .get(i % palette.len().max(1))
                .map(|c| (*c).to_owned())
                .unwrap_or_else(|| "#888888".into()),
        }
    }
}

// ------------------------------------------------------------------ pixel layout

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Fonts and strokes the layout needs to reserve room for (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutStyle {
    /// Tick label size.
    pub font: f32,
    /// Axis-title size.
    pub label_font: f32,
    pub line_width: f32,
    /// Average glyph advance as a fraction of the font size (Inter / SF ≈ 0.56).
    pub glyph_em: f32,
}

impl LayoutStyle {
    pub const MAC: LayoutStyle = LayoutStyle {
        font: 11.0,
        label_font: 10.5,
        line_width: 2.0,
        glyph_em: 0.56,
    };
    pub const IPHONE: LayoutStyle = LayoutStyle {
        font: 12.0,
        label_font: 11.5,
        line_width: 2.2,
        glyph_em: 0.56,
    };
    pub const PNG: LayoutStyle = LayoutStyle {
        font: 17.0,
        label_font: 16.0,
        line_width: 3.2,
        glyph_em: 0.56,
    };

    fn text_w(&self, s: &str, size: f32) -> f32 {
        s.chars().count() as f32 * size * self.glyph_em
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TickPos {
    pub pos: f32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SeriesShape {
    /// `pts[i]` is `None` at a gap. `lower` is the stack base path for an area (same length).
    Line {
        pts: Vec<Option<Pt>>,
        lower: Vec<Option<Pt>>,
        area: bool,
        style: LineStyle,
    },
    Scatter {
        pts: Vec<Pt>,
    },
    /// One rect per category index (`None` at a null value).
    Bars {
        rects: Vec<Option<Rect>>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesLayout {
    pub index: usize,
    pub name: String,
    pub color_index: usize,
    pub color: Option<String>,
    pub shape: SeriesShape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnnotationShape {
    Vline { x: f32, label: Option<String> },
    Hline { y: f32, label: Option<String> },
    Point { at: Pt, label: Option<String> },
    Band { rect: Rect, label: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slice {
    pub index: usize,
    /// Radians, clockwise from 12 o'clock.
    pub start: f32,
    pub end: f32,
    pub label: String,
    pub share: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieLayout {
    pub centre: Pt,
    pub radius: f32,
    pub slices: Vec<Slice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnPos {
    pub index: usize,
    pub x: f32,
    /// `(series index, y pixel)` for every series with a value in this column.
    pub points: Vec<(usize, f32)>,
}

/// Everything a painter needs, in the coordinate space of a `w × h` plot area (0,0 top-left).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlotLayout {
    pub w: f32,
    pub h: f32,
    /// The data rectangle (inside the axis labels).
    pub plot: Rect,
    pub x_ticks: Vec<TickPos>,
    pub y_ticks: Vec<TickPos>,
    pub y2_ticks: Vec<TickPos>,
    pub gridlines: Vec<f32>,
    pub series: Vec<SeriesLayout>,
    pub annotations: Vec<AnnotationShape>,
    pub x_label: Option<String>,
    pub y_label: Option<String>,
    pub y2_label: Option<String>,
    pub columns: Vec<ColumnPos>,
    pub pie: Option<PieLayout>,
    /// Pixel y of value 0 on the left axis (bar / area baseline), clamped to the plot.
    pub baseline: f32,
    pub font: f32,
    pub label_font: f32,
    pub line_width: f32,
}

/// What hover / touch-and-hold shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Readout {
    pub column: usize,
    /// Crosshair x (or the slice's mid-angle point for a pie).
    pub x: f32,
    pub label: String,
    pub entries: Vec<ReadoutEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadoutEntry {
    pub series: usize,
    pub name: String,
    pub color_index: usize,
    pub color: Option<String>,
    pub y: f32,
    pub text: String,
}

impl ChartModel {
    /// Lay the chart out in a `w × h` box.
    pub fn layout(&self, w: f32, h: f32, style: &LayoutStyle) -> PlotLayout {
        let font = style.font;
        if self.kind == Kind::Pie {
            return self.layout_pie(w, h, style);
        }
        let y_label_w = if self.y.label.is_some() {
            style.label_font * 1.4
        } else {
            0.0
        };
        let y_tick_w = self
            .y
            .ticks
            .iter()
            .map(|t| style.text_w(&t.label, font))
            .fold(0.0, f32::max);
        let pad_l = (y_label_w + y_tick_w + 8.0).max(font * 1.5).round();
        let y2_tick_w = self
            .y2
            .as_ref()
            .map(|a| {
                a.ticks
                    .iter()
                    .map(|t| style.text_w(&t.label, font))
                    .fold(0.0, f32::max)
                    + 8.0
            })
            .unwrap_or(0.0);
        let y2_label_w = self
            .y2
            .as_ref()
            .and_then(|a| a.label.as_ref())
            .map(|_| style.label_font * 1.4)
            .unwrap_or(0.0);
        let pad_r = (y2_tick_w + y2_label_w).max(font * 0.9).round();
        let pad_t = (font * 0.55).round();
        let pad_b = (font * 2.0
            + if self.x.label.is_some() {
                style.label_font * 1.3
            } else {
                0.0
            })
        .round();
        let plot = Rect {
            x: pad_l,
            y: pad_t,
            w: (w - pad_l - pad_r).max(10.0),
            h: (h - pad_t - pad_b).max(10.0),
        };
        let px = |v: f64| plot.x + self.x.frac(v) as f32 * plot.w;
        let py = |axis: &AxisModel, v: f64| plot.bottom() - axis.frac(v) as f32 * plot.h;
        let y_ticks: Vec<TickPos> = self
            .y
            .ticks
            .iter()
            .map(|t| TickPos {
                pos: py(&self.y, t.value),
                label: t.label.clone(),
            })
            .collect();
        let y2_ticks: Vec<TickPos> = self
            .y2
            .as_ref()
            .map(|a| {
                a.ticks
                    .iter()
                    .map(|t| TickPos {
                        pos: py(a, t.value),
                        label: t.label.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let gridlines = y_ticks.iter().map(|t| t.pos).collect();
        // x ticks: thin category labels so they never overlap.
        let mut x_ticks: Vec<TickPos> = self
            .x
            .ticks
            .iter()
            .map(|t| TickPos {
                pos: px(t.value),
                label: t.label.clone(),
            })
            .collect();
        if self.x.scale == Scale::Category && !x_ticks.is_empty() {
            let widest = x_ticks
                .iter()
                .map(|t| style.text_w(&t.label, font))
                .fold(0.0, f32::max)
                + font * 0.6;
            let fit = ((plot.w / widest).floor() as usize).max(1);
            if x_ticks.len() > fit {
                let every = (x_ticks.len() as f32 / fit as f32).ceil() as usize;
                x_ticks = x_ticks
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| i % every == 0)
                    .map(|(_, t)| t)
                    .collect();
            }
        }
        let baseline = py(&self.y, 0.0).clamp(plot.y, plot.bottom());
        let n_series = self.series.len().max(1);
        let n_cats = self.categories.len().max(1);
        let series = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let axis = match s.axis {
                    AxisSide::Left => &self.y,
                    AxisSide::Right => self.y2.as_ref().unwrap_or(&self.y),
                };
                let shape = match self.kind {
                    Kind::Line | Kind::Area => SeriesShape::Line {
                        pts: s
                            .points
                            .iter()
                            .map(|p| {
                                p.y.map(|y| Pt {
                                    x: px(p.x),
                                    y: py(axis, y),
                                })
                            })
                            .collect(),
                        lower: s
                            .points
                            .iter()
                            .map(|p| {
                                p.y.map(|_| Pt {
                                    x: px(p.x),
                                    y: if self.stacked {
                                        py(axis, p.y0)
                                    } else {
                                        baseline
                                    },
                                })
                            })
                            .collect(),
                        area: self.kind == Kind::Area,
                        style: s.style,
                    },
                    Kind::Scatter => SeriesShape::Scatter {
                        pts: s
                            .points
                            .iter()
                            .filter_map(|p| {
                                p.y.map(|y| Pt {
                                    x: px(p.x),
                                    y: py(axis, y),
                                })
                            })
                            .collect(),
                    },
                    Kind::Bar => {
                        let slot = plot.w / n_cats as f32;
                        let group_w = slot * 0.62;
                        let (bw, offset) = if self.stacked {
                            (group_w, 0.0)
                        } else {
                            let bw = group_w / n_series as f32;
                            (bw, i as f32 * bw)
                        };
                        let gap = (bw * 0.12).min(3.0);
                        SeriesShape::Bars {
                            rects: s
                                .points
                                .iter()
                                .map(|p| {
                                    let y = p.y?;
                                    let top = py(axis, y);
                                    let base = if self.stacked {
                                        py(axis, p.y0)
                                    } else {
                                        baseline
                                    };
                                    let x0 = plot.x + slot * (p.x as f32 + 0.5) - group_w / 2.0
                                        + offset
                                        + gap / 2.0;
                                    Some(Rect {
                                        x: x0,
                                        y: top.min(base),
                                        w: (bw - gap).max(1.0),
                                        h: (top - base).abs().max(0.5),
                                    })
                                })
                                .collect(),
                        }
                    }
                    Kind::Pie => unreachable!("pie laid out above"),
                };
                SeriesLayout {
                    index: i,
                    name: s.name.clone(),
                    color_index: s.color_index,
                    color: s.color.clone(),
                    shape,
                }
            })
            .collect();
        let annotations = self
            .annotations
            .iter()
            .filter_map(|a| {
                Some(match a.kind {
                    AnnotationKind::Vline => AnnotationShape::Vline {
                        x: px(a.x?),
                        label: a.label.clone(),
                    },
                    AnnotationKind::Hline => AnnotationShape::Hline {
                        y: py(&self.y, a.y?),
                        label: a.label.clone(),
                    },
                    AnnotationKind::Point => AnnotationShape::Point {
                        at: Pt {
                            x: px(a.x?),
                            y: py(&self.y, a.y?),
                        },
                        label: a.label.clone(),
                    },
                    AnnotationKind::Band => {
                        let (x0, x1) = (px(a.from?), px(a.to?));
                        AnnotationShape::Band {
                            rect: Rect {
                                x: x0.min(x1),
                                y: plot.y,
                                w: (x1 - x0).abs(),
                                h: plot.h,
                            },
                            label: a.label.clone(),
                        }
                    }
                })
            })
            .collect();
        let columns = self
            .columns
            .iter()
            .enumerate()
            .map(|(ci, c)| {
                let x = if self.kind == Kind::Bar {
                    plot.x + plot.w / n_cats as f32 * (c.x as f32 + 0.5)
                } else {
                    px(c.x)
                };
                let points = c
                    .entries
                    .iter()
                    .filter_map(|e| {
                        let s = &self.series[e.series];
                        let p = s.points.iter().find(|p| (p.x - c.x).abs() < 1e-9)?;
                        let axis = match s.axis {
                            AxisSide::Left => &self.y,
                            AxisSide::Right => self.y2.as_ref().unwrap_or(&self.y),
                        };
                        Some((e.series, py(axis, p.y?)))
                    })
                    .collect();
                ColumnPos {
                    index: ci,
                    x,
                    points,
                }
            })
            .collect();
        PlotLayout {
            w,
            h,
            plot,
            x_ticks,
            y_ticks,
            y2_ticks,
            gridlines,
            series,
            annotations,
            x_label: self.x.label.clone(),
            y_label: self.y.label.as_ref().map(|l| {
                if self.y.unit.is_empty() {
                    l.clone()
                } else {
                    format!("{l} ({})", self.y.unit)
                }
            }),
            y2_label: self.y2.as_ref().and_then(|a| {
                a.label.as_ref().map(|l| {
                    if a.unit.is_empty() {
                        l.clone()
                    } else {
                        format!("{l} ({})", a.unit)
                    }
                })
            }),
            columns,
            pie: None,
            baseline,
            font,
            label_font: style.label_font,
            line_width: style.line_width,
        }
    }

    fn layout_pie(&self, w: f32, h: f32, style: &LayoutStyle) -> PlotLayout {
        let radius = (w.min(h) / 2.0 - style.font * 0.6).max(10.0);
        let centre = Pt {
            x: w / 2.0,
            y: h / 2.0,
        };
        let mut angle = 0.0f32;
        let slices = self
            .shares
            .iter()
            .enumerate()
            .map(|(i, share)| {
                let sweep = (*share as f32) * std::f32::consts::TAU;
                let s = Slice {
                    index: i,
                    start: angle,
                    end: angle + sweep,
                    label: self.categories.get(i).cloned().unwrap_or_default(),
                    share: *share,
                };
                angle += sweep;
                s
            })
            .collect();
        PlotLayout {
            w,
            h,
            plot: Rect {
                x: centre.x - radius,
                y: centre.y - radius,
                w: radius * 2.0,
                h: radius * 2.0,
            },
            x_ticks: Vec::new(),
            y_ticks: Vec::new(),
            y2_ticks: Vec::new(),
            gridlines: Vec::new(),
            series: Vec::new(),
            annotations: Vec::new(),
            x_label: None,
            y_label: None,
            y2_label: None,
            columns: Vec::new(),
            pie: Some(PieLayout {
                centre,
                radius,
                slices,
            }),
            baseline: h,
            font: style.font,
            label_font: style.label_font,
            line_width: style.line_width,
        }
    }

    /// The readout for a pointer at `p` (plot-area coordinates), or `None` outside the plot.
    pub fn hit(&self, layout: &PlotLayout, p: Pt) -> Option<Readout> {
        if let Some(pie) = &layout.pie {
            let dx = p.x - pie.centre.x;
            let dy = p.y - pie.centre.y;
            if (dx * dx + dy * dy).sqrt() > pie.radius {
                return None;
            }
            let mut a = dx.atan2(-dy); // clockwise from 12 o'clock
            if a < 0.0 {
                a += std::f32::consts::TAU;
            }
            let slice = pie.slices.iter().find(|s| a >= s.start && a < s.end)?;
            let col = self.columns.get(slice.index)?;
            let mid = (slice.start + slice.end) / 2.0;
            let r = pie.radius * 0.6;
            return Some(Readout {
                column: slice.index,
                x: pie.centre.x + mid.sin() * r,
                label: col.label.clone(),
                entries: col
                    .entries
                    .iter()
                    .map(|e| ReadoutEntry {
                        series: e.series,
                        name: slice.label.clone(),
                        color_index: slice.index,
                        color: None,
                        y: pie.centre.y - mid.cos() * r,
                        text: e.text.clone(),
                    })
                    .collect(),
            });
        }
        let slack = layout.font * 1.2;
        if p.x < layout.plot.x - slack
            || p.x > layout.plot.right() + slack
            || p.y < layout.plot.y - slack
            || p.y > layout.plot.bottom() + slack
        {
            return None;
        }
        let cp = layout.columns.iter().min_by(|a, b| {
            (a.x - p.x)
                .abs()
                .partial_cmp(&(b.x - p.x).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let col = self.columns.get(cp.index)?;
        Some(Readout {
            column: cp.index,
            x: cp.x,
            label: col.label.clone(),
            entries: col
                .entries
                .iter()
                .filter_map(|e| {
                    let s = &self.series[e.series];
                    let y = cp
                        .points
                        .iter()
                        .find(|(si, _)| *si == e.series)
                        .map(|(_, y)| *y)?;
                    Some(ReadoutEntry {
                        series: e.series,
                        name: s.name.clone(),
                        color_index: s.color_index,
                        color: s.color.clone(),
                        y,
                        text: e.text.clone(),
                    })
                })
                .collect(),
        })
    }
}

// ------------------------------------------------------------------ table export

impl Chart {
    /// The table as cells: the header row, then one row per x / category (the Data view).
    pub fn table_cells(&self) -> (Vec<String>, Vec<Vec<String>>) {
        let model = self.model();
        let x_head = if self.kind.is_xy() {
            self.x.label.clone().unwrap_or_else(|| {
                if model.x.scale == Scale::Time {
                    "Date".into()
                } else {
                    "x".into()
                }
            })
        } else {
            self.x.label.clone().unwrap_or_else(|| "Category".into())
        };
        let unit_of = |s: &SeriesModel| match s.axis {
            AxisSide::Left => model.y.unit.clone(),
            AxisSide::Right => model
                .y2
                .as_ref()
                .map(|a| a.unit.clone())
                .unwrap_or_default(),
        };
        let mut head = vec![x_head];
        for s in &model.series {
            let u = unit_of(s);
            head.push(if u.is_empty() {
                s.name.clone()
            } else {
                format!("{} ({u})", s.name)
            });
        }
        if self.kind == Kind::Pie {
            head.push("Share".into());
        }
        let rows = model
            .columns
            .iter()
            .map(|c| {
                let x = if self.kind.is_xy() {
                    match model.x.scale {
                        Scale::Time => format_time_full(c.x),
                        Scale::Category => model.x.text(c.x),
                        _ => trim_float(c.x, 3),
                    }
                } else {
                    c.label.clone()
                };
                let mut row = vec![x];
                for (si, _) in model.series.iter().enumerate() {
                    row.push(
                        c.entries
                            .iter()
                            .find(|e| e.series == si)
                            .map(|e| trim_float(e.value, 3))
                            .unwrap_or_default(),
                    );
                }
                if self.kind == Kind::Pie {
                    let share = model.shares.get(c.x as usize).copied().unwrap_or(0.0);
                    row.push(format!("{}%", trim_float(share * 100.0, 1)));
                }
                row
            })
            .collect();
        (head, rows)
    }

    /// GitHub-flavoured markdown table of the data (the text-only fallback: push, search, quotes).
    pub fn table_markdown(&self) -> String {
        let (head, rows) = self.table_cells();
        let mut out = String::new();
        let esc = |s: &str| s.replace('|', "\\|");
        let _ = writeln!(
            out,
            "| {} |",
            head.iter().map(|h| esc(h)).collect::<Vec<_>>().join(" | ")
        );
        let _ = writeln!(
            out,
            "|{}|",
            head.iter().map(|_| " --- ").collect::<Vec<_>>().join("|")
        );
        for r in rows {
            let _ = writeln!(
                out,
                "| {} |",
                r.iter().map(|c| esc(c)).collect::<Vec<_>>().join(" | ")
            );
        }
        out.trim_end().to_owned()
    }

    /// Tab-separated values with a header row ("Copy data").
    pub fn table_tsv(&self) -> String {
        let (head, rows) = self.table_cells();
        let mut out = head.join("\t");
        for r in rows {
            out.push('\n');
            out.push_str(&r.join("\t"));
        }
        out
    }

    /// Total number of data points (xy) or values (categorical).
    pub fn point_count(&self) -> usize {
        self.series
            .iter()
            .map(|s| s.points.len() + s.values.len())
            .sum()
    }

    /// The push / list-preview line for a chart message without a caption.
    pub fn preview_text(&self) -> String {
        match &self.title {
            Some(t) if !t.trim().is_empty() => format!("Chart: {}", t.trim()),
            _ => "Chart".into(),
        }
    }
}

/// The series palette per theme: the code-card token colours (keyword · function · string ·
/// number), then accent and online — contrast-checked on both bubbles already.
pub const PALETTE_DARK: [&str; 6] = [
    "#95D0CD", "#E6B387", "#D898D8", "#F1C972", "#1084FE", "#34C759",
];
pub const PALETTE_LIGHT: [&str; 6] = [
    "#1F7F7A", "#B8641A", "#A63FA4", "#9A6F00", "#0A6FE0", "#248A3D",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval_chart() -> Value {
        let runs: Vec<f64> = (1..=20).map(|r| r as f64).collect();
        let affine = [
            66.2, 68.0, 69.1, 70.4, 70.9, 71.6, 71.0, 71.8, 72.9, 74.1, 75.0, 79.6, 81.8, 83.4,
            82.9, 84.0, 83.6, 84.7, 85.1, 85.4,
        ];
        let base = [
            70.1, 69.4, 71.0, 70.6, 69.8, 71.3, 70.2, 71.8, 70.9, 71.5, 70.4, 71.1, 71.6, 71.2,
            70.8, 71.9, 71.4, 70.7, 71.6, 71.3,
        ];
        json!({
            "kind": "line",
            "title": "Eval score over runs",
            "subtitle": "memory-pack eval · 240 tasks per run",
            "series": [
                { "name": "Affine 2", "points": runs.iter().zip(affine).map(|(r, v)| json!([r, v])).collect::<Vec<_>>() },
                { "name": "Baseline", "points": runs.iter().zip(base).map(|(r, v)| json!([r, v])).collect::<Vec<_>>() }
            ],
            "x": { "label": "Run" },
            "y": { "label": "Pass rate", "unit": "%", "min": 60, "max": 90 },
            "annotations": [
                { "type": "vline", "x": 12, "label": "prompt v3" },
                { "type": "hline", "y": 80, "label": "target" }
            ]
        })
    }

    fn bar_chart() -> Value {
        json!({
            "kind": "bar",
            "title": "Pass rate by task family",
            "categories": ["Recall", "Planning", "Code", "Tone"],
            "series": [
                { "name": "Affine 2", "values": [86, 80, 78, 81] },
                { "name": "Baseline", "values": [74, 61, 69, 80] }
            ],
            "y": { "label": "Pass rate", "unit": "%", "min": 0, "max": 100 }
        })
    }

    #[test]
    fn the_schema_is_valid_json_with_the_caps() {
        let s: Value = serde_json::from_str(SCHEMA_JSON).unwrap();
        assert_eq!(s["properties"]["series"]["maxItems"], json!(MAX_SERIES));
        assert_eq!(
            s["properties"]["series"]["items"]["properties"]["points"]["maxItems"],
            json!(MAX_POINTS_PER_SERIES)
        );
        assert_eq!(
            s["properties"]["categories"]["maxItems"],
            json!(MAX_CATEGORIES)
        );
        assert_eq!(
            s["properties"]["annotations"]["maxItems"],
            json!(MAX_ANNOTATIONS)
        );
        assert_eq!(s["properties"]["kind"]["enum"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn the_eval_chart_parses_and_round_trips() {
        let c = Chart::parse(&eval_chart()).unwrap();
        assert_eq!(c.kind, Kind::Line);
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.point_count(), 40);
        let back: Chart = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert_eq!(c.preview_text(), "Chart: Eval score over runs");
    }

    #[test]
    fn caps_refuse_with_the_downsample_hint() {
        let mut v = eval_chart();
        let big: Vec<Value> = (0..501).map(|i| json!([i, i])).collect();
        v["series"][0]["points"] = json!(big);
        let e = Chart::parse(&v).unwrap_err();
        assert!(
            e.contains("501 points") && e.contains("deliver_file"),
            "{e}"
        );

        let mut v = eval_chart();
        let pts: Vec<Value> = (0..400).map(|i| json!([i, i])).collect();
        v["series"] = json!((0..6)
            .map(|i| json!({ "name": format!("s{i}"), "points": pts }))
            .collect::<Vec<_>>());
        let e = Chart::parse(&v).unwrap_err();
        assert!(e.contains("2400 points") && e.contains("2000"), "{e}");

        let mut v = eval_chart();
        v["series"] = json!((0..9)
            .map(|i| json!({ "name": format!("s{i}"), "points": [[1, 1]] }))
            .collect::<Vec<_>>());
        assert!(Chart::parse(&v).unwrap_err().contains("9 series"));

        let mut v = bar_chart();
        v["categories"] = json!((0..101).map(|i| i.to_string()).collect::<Vec<_>>());
        v["series"] = json!([{ "name": "a", "values": vec![1; 101] }]);
        assert!(Chart::parse(&v).unwrap_err().contains("101 categories"));

        // Over 32 KB with long labels, under every count cap.
        let mut v = eval_chart();
        v["series"][0]["points"] = json!((0..500)
            .map(|i| json!([i as f64 + 0.123456789012345, i as f64 + 0.987654321098765]))
            .collect::<Vec<_>>());
        v["series"][1]["points"] = json!((0..500)
            .map(|i| json!([i as f64 + 0.123456789012345, i as f64 + 0.987654321098765]))
            .collect::<Vec<_>>());
        v["series"].as_array_mut().unwrap().push(json!({ "name": "c", "points": (0..500).map(|i| json!([i as f64 + 0.123456789012345, i as f64 + 0.987654321098765])).collect::<Vec<_>>() }));
        let e = Chart::parse(&v).unwrap_err();
        assert!(e.contains("bytes") && e.contains("32768"), "{e}");

        let mut v = eval_chart();
        v["title"] = json!("x".repeat(81));
        assert!(Chart::parse(&v).unwrap_err().contains("title"));
    }

    #[test]
    fn kind_rules_are_explained() {
        let mut v = bar_chart();
        v["series"][0]["values"] = json!([1, 2]);
        assert!(Chart::parse(&v)
            .unwrap_err()
            .contains("2 values for 4 categories"));
        let mut v = bar_chart();
        v["categories"] = json!([]);
        assert!(Chart::parse(&v).unwrap_err().contains("need categories"));
        let mut v = eval_chart();
        v["series"][0] = json!({ "name": "a", "values": [1, 2, 3] });
        assert!(Chart::parse(&v).unwrap_err().contains("points"));
        let mut v = eval_chart();
        v["bogus"] = json!(1);
        assert!(Chart::parse(&v).unwrap_err().contains("unknown field"));
        let mut v = eval_chart();
        v["series"][0]["color"] = json!("red");
        assert!(Chart::parse(&v).unwrap_err().contains("#RRGGBB"));
        let mut v = eval_chart();
        v["annotations"] = json!([{ "type": "vline" }]);
        assert!(Chart::parse(&v).unwrap_err().contains("needs x"));
        let pie = json!({ "kind": "pie", "categories": ["a", "b"], "series": [{ "name": "s", "values": [1, 3] }, { "name": "t", "values": [1, 1] }] });
        assert!(Chart::parse(&pie)
            .unwrap_err()
            .contains("exactly one series"));
        let pie = json!({ "kind": "pie", "categories": (0..13).map(|i| i.to_string()).collect::<Vec<_>>(), "series": [{ "name": "s", "values": vec![1; 13] }] });
        assert!(Chart::parse(&pie).unwrap_err().contains("12 slices"));
        let log = json!({ "kind": "line", "series": [{ "name": "s", "points": [[1, 0], [2, 5]] }], "y": { "scale": "log" } });
        assert!(Chart::parse(&log).unwrap_err().contains("above 0"));
        let txt =
            json!({ "kind": "line", "series": [{ "name": "s", "points": [["a", 1], [2, 5]] }] });
        assert!(Chart::parse(&txt).unwrap_err().contains("x.scale"));
        let bad_date = json!({ "kind": "line", "x": { "scale": "time" }, "series": [{ "name": "s", "points": [["yesterday", 1]] }] });
        assert!(Chart::parse(&bad_date).unwrap_err().contains("ISO date"));
    }

    #[test]
    fn the_model_resolves_domains_ticks_and_columns() {
        let m = Chart::parse(&eval_chart()).unwrap().model();
        assert_eq!((m.y.min, m.y.max), (60.0, 90.0));
        assert_eq!(
            m.y.ticks
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["60%", "70%", "80%", "90%"]
        );
        assert_eq!((m.x.min, m.x.max), (1.0, 20.0));
        assert_eq!(
            m.x.ticks
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["1", "5", "10", "15", "20"]
        );
        assert!(m.show_legend);
        assert_eq!(m.columns.len(), 20);
        let c = &m.columns[13];
        assert_eq!(c.label, "Run 14");
        assert_eq!(c.entries[0].text, "83.4%");
        assert_eq!(c.entries[1].text, "71.2%");
        assert_eq!(m.column_near(13.8), Some(13));
        assert_eq!(m.column_near(40.0), None);
        assert_eq!(m.annotations[0].x, Some(12.0));
        assert_eq!(m.annotations[1].y, Some(80.0));
        assert_eq!(m.series_color(0, &PALETTE_DARK), "#95D0CD");
        assert_eq!(m.series_color(7, &PALETTE_DARK), "#E6B387");
    }

    #[test]
    fn auto_domains_are_nice_and_bars_start_at_zero() {
        let c = Chart::parse(&json!({ "kind": "line", "series": [{ "name": "a", "points": [[0, 12.4], [1, 17.9], [2, 15.1]] }], "y": { "unit": "ms" } })).unwrap();
        let m = c.model();
        assert_eq!((m.y.min, m.y.max), (12.0, 18.0));
        assert_eq!(
            m.y.ticks
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["12 ms", "14 ms", "16 ms", "18 ms"]
        );
        let b = Chart::parse(&json!({ "kind": "bar", "categories": ["a", "b"], "series": [{ "name": "s", "values": [3.5, 9.2] }] })).unwrap().model();
        assert_eq!(b.y.min, 0.0);
        assert!(b.y.max >= 9.2);
        assert_eq!(b.x.scale, Scale::Category);
        assert_eq!(b.columns[1].label, "b");
        assert_eq!(b.columns[1].entries[0].text, "9.2");
        assert!(!b.show_legend, "one series: no legend");
    }

    #[test]
    fn stacked_series_sit_on_each_other_and_read_their_own_value() {
        let c = Chart::parse(&json!({ "kind": "bar", "stacked": true, "categories": ["q1", "q2"], "series": [{ "name": "a", "values": [10, 20] }, { "name": "b", "values": [5, 5] }] })).unwrap();
        let m = c.model();
        assert_eq!(m.series[1].points[0].y0, 10.0);
        assert_eq!(m.series[1].points[0].y, Some(15.0));
        assert_eq!(m.columns[0].entries[1].text, "5");
        assert!(m.y.max >= 25.0);
    }

    #[test]
    fn time_and_log_axes() {
        let c = Chart::parse(&json!({ "kind": "area", "series": [{ "name": "users", "points": [["2026-01-01", 10], ["2026-02-01", 30], ["2026-03-01", 25]] }], "y": { "format": "compact" } })).unwrap();
        let m = c.model();
        assert_eq!(m.x.scale, Scale::Time);
        assert!(m.x.ticks.len() >= 2, "{:?}", m.x.ticks);
        assert_eq!(m.columns[0].label, "2026-01-01");
        assert_eq!(parse_time("2026-10-26T14:00:00Z"), Some(1_793_023_200.0));
        assert_eq!(
            parse_time("2026-10-26 14:00"),
            parse_time("2026-10-26T14:00:00Z")
        );
        assert_eq!(parse_time("2026-10"), parse_time("2026-10-01"));
        assert_eq!(
            format_time(parse_time("2026-03-01").unwrap(), 60.0 * 86400.0),
            "1 Mar"
        );
        let l = Chart::parse(&json!({ "kind": "scatter", "series": [{ "name": "s", "points": [[1, 10], [2, 1000], [3, 100000]] }], "y": { "scale": "log" } })).unwrap().model();
        assert_eq!(
            l.y.ticks.iter().map(|t| t.value).collect::<Vec<_>>(),
            [10.0, 100.0, 1000.0, 10000.0, 100000.0]
        );
        assert!((l.y.frac(1000.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn value_formats() {
        assert_eq!(format_value(83.4, Format::Percent, "%", None), "83.4%");
        assert_eq!(format_value(70.0, Format::Percent, "%", Some(0)), "70%");
        assert_eq!(format_value(12.5, Format::Auto, "ms", None), "12.5 ms");
        assert_eq!(format_value(12345.678, Format::Auto, "", None), "12,345.7");
        assert_eq!(format_value(1234567.0, Format::Compact, "", None), "1.23M");
        assert_eq!(format_value(1500.0, Format::Compact, "", None), "1.5k");
        assert_eq!(format_value(1234.5, Format::Currency, "$", None), "$1,235");
        assert_eq!(format_value(-12.5, Format::Currency, "€", None), "-€12.5");
        assert_eq!(format_value(99.0, Format::Currency, "USD", None), "$99 USD");
        assert_eq!(format_value(45.0, Format::Duration, "", None), "45 s");
        assert_eq!(format_value(90.0, Format::Duration, "", None), "1m 30s");
        assert_eq!(format_value(3900.0, Format::Duration, "", None), "1h 05m");
        assert_eq!(format_value(2.0, Format::Integer, "", None), "2");
        assert_eq!(nice_step(60.0, 90.0, 4), 10.0);
        assert_eq!(nice_step(0.0, 1.0, 4), 0.25);
        assert_eq!(ticks_in(60.0, 90.0, 10.0), [60.0, 70.0, 80.0, 90.0]);
    }

    #[test]
    fn the_layout_places_the_plot_and_the_columns() {
        let c = Chart::parse(&eval_chart()).unwrap();
        let m = c.model();
        let l = m.layout(510.0, 210.0, &LayoutStyle::MAC);
        assert!(
            l.plot.x > 40.0 && l.plot.x < 80.0,
            "left pad for '90%' + the rotated label: {}",
            l.plot.x
        );
        assert!(l.plot.bottom() < 210.0 - 30.0, "room for ticks and 'Run'");
        assert_eq!(l.y_ticks.len(), 4);
        assert!(
            (l.y_ticks[0].pos - l.plot.bottom()).abs() < 0.01,
            "60% is the bottom edge"
        );
        assert!(
            (l.y_ticks[3].pos - l.plot.y).abs() < 0.01,
            "90% is the top edge"
        );
        assert_eq!(l.columns.len(), 20);
        assert!(
            (l.columns[0].x - l.plot.x).abs() < 0.01
                && (l.columns[19].x - l.plot.right()).abs() < 0.01
        );
        let SeriesShape::Line { pts, area, .. } = &l.series[0].shape else {
            panic!()
        };
        assert_eq!(pts.len(), 20);
        assert!(!area);
        // 83.4 % on a 60–90 axis sits 78 % of the way up.
        let y = pts[13].unwrap().y;
        let expect = l.plot.bottom() - 0.78 * l.plot.h;
        assert!((y - expect).abs() < 0.5, "{y} vs {expect}");
        assert!(matches!(l.annotations[0], AnnotationShape::Vline { .. }));
        assert!(matches!(l.annotations[1], AnnotationShape::Hline { .. }));
        assert_eq!(l.y_label.as_deref(), Some("Pass rate (%)"));
    }

    #[test]
    fn hover_reads_the_nearest_column_and_nothing_outside() {
        let c = Chart::parse(&eval_chart()).unwrap();
        let m = c.model();
        let l = m.layout(510.0, 210.0, &LayoutStyle::MAC);
        let x14 = l.columns[13].x;
        let r = m
            .hit(
                &l,
                Pt {
                    x: x14 + 3.0,
                    y: l.plot.y + 20.0,
                },
            )
            .expect("inside");
        assert_eq!(r.column, 13);
        assert_eq!(r.label, "Run 14");
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.entries[0].name, "Affine 2");
        assert_eq!(r.entries[0].text, "83.4%");
        assert_eq!(r.entries[1].text, "71.2%");
        assert!((r.x - x14).abs() < 0.01);
        assert!(m.hit(&l, Pt { x: 2.0, y: 2.0 }).is_none());
        assert!(m
            .hit(
                &l,
                Pt {
                    x: l.plot.x + 10.0,
                    y: l.h + 40.0
                }
            )
            .is_none());
        // Bars: the column is the category slot's centre.
        let b = Chart::parse(&bar_chart()).unwrap();
        let bm = b.model();
        let bl = bm.layout(510.0, 200.0, &LayoutStyle::MAC);
        let SeriesShape::Bars { rects } = &bl.series[0].shape else {
            panic!()
        };
        assert_eq!(rects.len(), 4);
        let r = bm
            .hit(
                &bl,
                Pt {
                    x: bl.columns[1].x,
                    y: bl.plot.y + 10.0,
                },
            )
            .unwrap();
        assert_eq!(r.label, "Planning");
        assert_eq!(r.entries[0].text, "80%");
        assert_eq!(r.entries[1].text, "61%");
    }

    #[test]
    fn pie_slices_and_hit() {
        let p = Chart::parse(&json!({ "kind": "pie", "title": "Spend", "categories": ["Inference", "Boxes", "Other"], "series": [{ "name": "usd", "values": [50, 30, 20] }], "y": { "unit": "$", "format": "currency" } })).unwrap();
        let m = p.model();
        assert_eq!(m.shares, [0.5, 0.3, 0.2]);
        assert_eq!(m.columns[0].entries[0].text, "$50 · 50%");
        let l = m.layout(300.0, 300.0, &LayoutStyle::MAC);
        let pie = l.pie.as_ref().unwrap();
        assert_eq!(pie.slices.len(), 3);
        assert!((pie.slices[0].end - std::f32::consts::PI).abs() < 1e-5);
        // A point just right of the centre, slightly up: 3 o'clock = the first slice (0..180°).
        let r = m
            .hit(
                &l,
                Pt {
                    x: pie.centre.x + 40.0,
                    y: pie.centre.y - 5.0,
                },
            )
            .unwrap();
        assert_eq!(r.label, "Inference");
        // 9 o'clock (270°) is still "Boxes" (180°..288°); 330° is inside "Other" (288°..360°).
        let r = m
            .hit(
                &l,
                Pt {
                    x: pie.centre.x - 40.0,
                    y: pie.centre.y - 5.0,
                },
            )
            .unwrap();
        assert_eq!(r.label, "Boxes");
        let r = m
            .hit(
                &l,
                Pt {
                    x: pie.centre.x - 20.0,
                    y: pie.centre.y - 35.0,
                },
            )
            .unwrap();
        assert_eq!(r.label, "Other");
        assert!(m.hit(&l, Pt { x: 1.0, y: 1.0 }).is_none());
        assert!(p.table_markdown().contains("| Share |"));
    }

    #[test]
    fn table_exports() {
        let c = Chart::parse(&eval_chart()).unwrap();
        let md = c.table_markdown();
        let lines: Vec<&str> = md.lines().collect();
        assert_eq!(lines[0], "| Run | Affine 2 (%) | Baseline (%) |");
        assert_eq!(lines[1], "| --- | --- | --- |");
        assert_eq!(lines[2], "| 1 | 66.2 | 70.1 |");
        assert_eq!(lines.len(), 22);
        let tsv = c.table_tsv();
        assert_eq!(
            tsv.lines().next().unwrap(),
            "Run\tAffine 2 (%)\tBaseline (%)"
        );
        assert_eq!(tsv.lines().nth(14).unwrap(), "14\t83.4\t71.2");
        let b = Chart::parse(&bar_chart()).unwrap();
        assert_eq!(b.table_tsv().lines().nth(2).unwrap(), "Planning\t80\t61");
    }

    #[test]
    fn gaps_and_category_x_on_a_line_chart() {
        let c = Chart::parse(&json!({ "kind": "line", "series": [{ "name": "s", "points": [["Mon", 1], ["Tue", null], ["Wed", 3]] }] })).unwrap();
        let m = c.model();
        assert_eq!(m.x.scale, Scale::Category);
        assert_eq!(m.x.categories, ["Mon", "Tue", "Wed"]);
        assert_eq!(m.columns.len(), 3);
        assert!(
            m.columns[1].entries.is_empty(),
            "a null is a gap, not a zero"
        );
        let l = m.layout(300.0, 150.0, &LayoutStyle::IPHONE);
        let SeriesShape::Line { pts, .. } = &l.series[0].shape else {
            panic!()
        };
        assert!(pts[1].is_none());
        assert_eq!(
            l.x_ticks
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>(),
            ["Mon", "Tue", "Wed"]
        );
    }
}
