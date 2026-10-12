//! Offsets that change with zoom, like MapLibre's `fill-translate`.

use serde_json::Value as Json;

/// An `[x, y]` offset in CSS pixels by MapLibre zoom: a constant, an
/// `interpolate` expression on `["zoom"]` of `["literal", [x, y]]` values,
/// or legacy `{"stops": [[zoom, [x, y]], ...], "base": b}`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OffsetCurve {
    /// `(zoom, (x, y))` pairs in increasing zoom order.
    stops: Vec<(f64, (f64, f64))>,
    /// Exponential base between stops (`1` is linear).
    base: f64,
}

impl OffsetCurve {
    /// Parse an offset or a zoom curve of offsets.
    pub(crate) fn parse(json: &Json) -> Result<Self, String> {
        let bad = || format!("unsupported offset `{json}` (need [x, y] or a zoom curve of them)");
        if let Some(at) = pair(json) {
            return Ok(Self {
                stops: vec![(0.0, at)],
                base: 1.0,
            });
        }
        let (stops, base) = if let Some(object) = json.as_object() {
            let stops = object
                .get("stops")
                .and_then(Json::as_array)
                .ok_or_else(bad)?
                .iter()
                .map(|s| Some((s.get(0)?.as_f64()?, pair(s.get(1)?)?)))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(bad)?;
            (
                stops,
                object.get("base").and_then(Json::as_f64).unwrap_or(1.0),
            )
        } else {
            let items = json.as_array().ok_or_else(bad)?;
            let zoomed = items.len() >= 5
                && items[0] == "interpolate"
                && items[2] == serde_json::json!(["zoom"]);
            if !zoomed {
                return Err(bad());
            }
            let base = match items[1].as_array().map(Vec::as_slice) {
                Some([Json::String(k), ..]) if k == "linear" => 1.0,
                Some([Json::String(k), b]) if k == "exponential" => b.as_f64().ok_or_else(bad)?,
                _ => return Err(bad()),
            };
            let stops = items[3..]
                .chunks(2)
                .map(|p| Some((p.first()?.as_f64()?, pair(p.get(1)?)?)))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(bad)?;
            (stops, base)
        };
        let ordered = stops.windows(2).all(|w| w[0].0 < w[1].0);
        if stops.is_empty() || !ordered || base <= 0.0 {
            return Err(bad());
        }
        Ok(Self { stops, base })
    }

    /// The offset at `zoom` (held past the first and last stops).
    pub(crate) fn at(&self, zoom: f64) -> (f64, f64) {
        let i = self.stops.partition_point(|s| s.0 <= zoom);
        if i == 0 {
            return self.stops[0].1;
        }
        let (z0, a) = self.stops[i - 1];
        let Some(&(z1, b)) = self.stops.get(i) else {
            return a;
        };
        let t = if (self.base - 1.0).abs() < 1e-9 {
            (zoom - z0) / (z1 - z0)
        } else {
            (self.base.powf(zoom - z0) - 1.0) / (self.base.powf(z1 - z0) - 1.0)
        };
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }

    /// Whether the offset is `[0, 0]` at every zoom.
    pub(crate) fn is_zero(&self) -> bool {
        self.stops.iter().all(|s| s.1 == (0.0, 0.0))
    }

    /// The curve as JSON [`OffsetCurve::parse`] reads back: `[x, y]` when it
    /// never changes, else legacy stops.
    pub(crate) fn to_json(&self) -> Json {
        if let [(_, (x, y))] = self.stops.as_slice() {
            return serde_json::json!([x, y]);
        }
        let stops: Vec<Json> = self
            .stops
            .iter()
            .map(|&(z, (x, y))| serde_json::json!([z, [x, y]]))
            .collect();
        serde_json::json!({ "stops": stops, "base": self.base })
    }
}

/// `[x, y]` or `["literal", [x, y]]` as finite numbers.
fn pair(json: &Json) -> Option<(f64, f64)> {
    let values = match json.as_array().map(Vec::as_slice) {
        Some([Json::String(op), inner]) if op == "literal" => inner,
        _ => json,
    };
    match values.as_array().map(Vec::as_slice) {
        Some([x, y]) => {
            Some((x.as_f64()?, y.as_f64()?)).filter(|p| p.0.is_finite() && p.1.is_finite())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offsets_follow_zoom_and_round_trip() {
        let curve = OffsetCurve::parse(&json!([
            "interpolate",
            ["linear"],
            ["zoom"],
            14,
            ["literal", [0, 0]],
            16,
            ["literal", [-2, -2]]
        ]))
        .unwrap();
        assert_eq!(curve.at(13.0), (0.0, 0.0));
        assert_eq!(curve.at(15.0), (-1.0, -1.0));
        assert_eq!(curve.at(17.0), (-2.0, -2.0));
        assert!(!curve.is_zero());
        let constant = OffsetCurve::parse(&json!([0, 0])).unwrap();
        assert!(constant.is_zero());
        for c in [curve, constant] {
            assert_eq!(OffsetCurve::parse(&c.to_json()).unwrap(), c);
        }
        assert!(OffsetCurve::parse(&json!([1])).is_err());
        assert!(OffsetCurve::parse(&json!(["get", "x"])).is_err());
    }
}
