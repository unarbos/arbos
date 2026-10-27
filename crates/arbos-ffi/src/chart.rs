//! Chart bubbles for Swift (`arbos_core::chart`): the iPhone validates a `chart` part, draws
//! Swift Charts from the resolved model (domains, ticks, formatted values, readout columns),
//! and copies the data with the exact rules the Mac and agentd use. JSON in, JSON out: the
//! model is a tree of records Swift decodes with `Codable` (snake_case keys).

use arbos_core::chart::{Chart, LayoutStyle, Pt};

use crate::FfiError;

fn parse(spec_json: &str) -> Result<Chart, FfiError> {
    let v: serde_json::Value =
        serde_json::from_str(spec_json).map_err(|e| FfiError::Decode(e.to_string()))?;
    Chart::parse(&v).map_err(FfiError::Decode)
}

/// `None` when the spec is a valid chart, else the reason (the text agentd would refuse with).
#[uniffi::export]
pub fn chart_validate(spec_json: String) -> Option<String> {
    parse(&spec_json).err().map(|e| e.to_string())
}

/// The resolved [`arbos_core::chart::ChartModel`] as JSON.
#[uniffi::export]
pub fn chart_model_json(spec_json: String) -> Result<String, FfiError> {
    let model = parse(&spec_json)?.model();
    serde_json::to_string(&model).map_err(|e| FfiError::Decode(e.to_string()))
}

/// Pixel geometry for a `width × height` plot (iPhone style: 12 pt ticks) as JSON — for
/// surfaces that paint by hand (image export) rather than through Swift Charts.
#[uniffi::export]
pub fn chart_layout_json(spec_json: String, width: f32, height: f32) -> Result<String, FfiError> {
    let model = parse(&spec_json)?.model();
    let layout = model.layout(width, height, &LayoutStyle::IPHONE);
    serde_json::to_string(&layout).map_err(|e| FfiError::Decode(e.to_string()))
}

/// The readout under a touch at `(x, y)` in a `width × height` plot, as JSON; `None` outside.
#[uniffi::export]
pub fn chart_hit_json(
    spec_json: String,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
) -> Result<Option<String>, FfiError> {
    let model = parse(&spec_json)?.model();
    let layout = model.layout(width, height, &LayoutStyle::IPHONE);
    match model.hit(&layout, Pt { x, y }) {
        Some(r) => serde_json::to_string(&r)
            .map(Some)
            .map_err(|e| FfiError::Decode(e.to_string())),
        None => Ok(None),
    }
}

/// Tab-separated data with a header row ("Copy data").
#[uniffi::export]
pub fn chart_table_tsv(spec_json: String) -> Result<String, FfiError> {
    Ok(parse(&spec_json)?.table_tsv())
}

/// The markdown table (quotes, search, text-only fallback).
#[uniffi::export]
pub fn chart_table_markdown(spec_json: String) -> Result<String, FfiError> {
    Ok(parse(&spec_json)?.table_markdown())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"{"kind":"line","title":"Eval","series":[{"name":"a","points":[[1,70],[2,80],[3,85]]},{"name":"b","points":[[1,71],[2,71.5],[3,71]]}],"x":{"label":"Run"},"y":{"unit":"%","min":60,"max":90}}"#;

    #[test]
    fn the_swift_surface_round_trips_the_model() {
        assert_eq!(chart_validate(SPEC.into()), None);
        assert!(chart_validate("{\"kind\":\"pie\"}".into())
            .unwrap()
            .contains("series"));
        assert!(chart_validate("not json".into()).is_some());
        let m: serde_json::Value =
            serde_json::from_str(&chart_model_json(SPEC.into()).unwrap()).unwrap();
        assert_eq!(m["kind"], "line");
        assert_eq!(m["columns"].as_array().unwrap().len(), 3);
        assert_eq!(m["columns"][1]["entries"][0]["text"], "80%");
        assert_eq!(m["y"]["ticks"][0]["label"], "60%");
        let l: serde_json::Value =
            serde_json::from_str(&chart_layout_json(SPEC.into(), 300.0, 200.0).unwrap()).unwrap();
        assert_eq!(l["columns"].as_array().unwrap().len(), 3);
        let x1 = l["columns"][1]["x"].as_f64().unwrap() as f32;
        let hit = chart_hit_json(SPEC.into(), 300.0, 200.0, x1, 50.0)
            .unwrap()
            .unwrap();
        let h: serde_json::Value = serde_json::from_str(&hit).unwrap();
        assert_eq!(h["label"], "Run 2");
        assert_eq!(h["entries"][0]["text"], "80%");
        assert_eq!(
            chart_hit_json(SPEC.into(), 300.0, 200.0, 1.0, 1.0).unwrap(),
            None
        );
        assert_eq!(
            chart_table_tsv(SPEC.into())
                .unwrap()
                .lines()
                .nth(1)
                .unwrap(),
            "1\t70\t71"
        );
        assert!(chart_table_markdown(SPEC.into())
            .unwrap()
            .starts_with("| Run | a (%) | b (%) |"));
    }
}
