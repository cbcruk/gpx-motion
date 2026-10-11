//! The parts of the MapLibre style expression language a map style needs:
//! feature filters and zoom curves.

use serde_json::Value as Json;

use super::mvt::{Feature, GeomType, Layer, Value};

/// A feature filter in MapLibre syntax (legacy or expression form), kept
/// alongside its source JSON so a style can be written back out.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    test: Test,
    json: Json,
}

#[derive(Debug, Clone, PartialEq)]
enum Test {
    Const(bool),
    All(Vec<Test>),
    Any(Vec<Test>),
    Not(Box<Test>),
    /// `input op value` (`==`, `!=`, `<`, `<=`, `>`, `>=`).
    Compare(Input, Op, Json),
    /// `input` is one of `values` (`in`, `match`).
    OneOf(Input, Vec<Json>),
    Has(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Input {
    Property(String),
    GeometryType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Filter {
    /// Parse a MapLibre filter, or say which part isn't supported.
    pub fn parse(json: &Json) -> Result<Self, String> {
        Ok(Self {
            test: test(json)?,
            json: json.clone(),
        })
    }

    /// The filter's source JSON.
    pub fn json(&self) -> &Json {
        &self.json
    }

    /// Whether a feature of `layer` passes.
    pub fn accepts(&self, layer: &Layer, feature: &Feature) -> bool {
        eval(&self.test, layer, feature)
    }
}

fn test(json: &Json) -> Result<Test, String> {
    if let Some(b) = json.as_bool() {
        return Ok(Test::Const(b));
    }
    let items = json
        .as_array()
        .ok_or_else(|| format!("unsupported filter `{json}`"))?;
    let (op, args) = items
        .split_first()
        .and_then(|(op, args)| Some((op.as_str()?, args)))
        .ok_or_else(|| format!("unsupported filter `{json}`"))?;
    let all = |args: &[Json]| args.iter().map(test).collect::<Result<Vec<_>, _>>();
    let compare = |op| -> Result<Test, String> {
        let [input, value] = args else {
            return Err(format!("`{json}` needs two arguments"));
        };
        Ok(Test::Compare(self::input(input)?, op, literal(value)?))
    };
    Ok(match op {
        "all" => Test::All(all(args)?),
        "any" => Test::Any(all(args)?),
        "none" => Test::Not(Box::new(Test::Any(all(args)?))),
        "!" => match args {
            [inner] => Test::Not(Box::new(test(inner)?)),
            _ => return Err(format!("`{json}` needs one argument")),
        },
        "==" => compare(Op::Eq)?,
        "!=" => compare(Op::Ne)?,
        "<" => compare(Op::Lt)?,
        "<=" => compare(Op::Le)?,
        ">" => compare(Op::Gt)?,
        ">=" => compare(Op::Ge)?,
        // Expression form: `["in", ["get", key], ["literal", [values...]]]`.
        "in" if args.first().is_some_and(Json::is_array) => {
            let [needle, haystack] = args else {
                return Err(format!("unsupported filter `{json}`"));
            };
            let values = match literal(haystack)? {
                Json::Array(values) => values,
                _ => return Err(format!("unsupported filter `{json}`")),
            };
            Test::OneOf(self::input(needle)?, values)
        }
        "in" | "!in" => {
            let (input, values) = args
                .split_first()
                .ok_or_else(|| format!("`{json}` needs arguments"))?;
            if input.is_array() {
                return Err(format!("unsupported filter `{json}`"));
            }
            let values = values.iter().map(literal).collect::<Result<_, _>>()?;
            let one_of = Test::OneOf(self::input(input)?, values);
            if op == "in" {
                one_of
            } else {
                Test::Not(Box::new(one_of))
            }
        }
        "has" | "!has" => {
            let key = match args {
                [Json::String(key)] => key.clone(),
                _ => return Err(format!("unsupported filter `{json}`")),
            };
            if op == "has" {
                Test::Has(key)
            } else {
                Test::Not(Box::new(Test::Has(key)))
            }
        }
        "match" => match_test(json, args)?,
        _ => return Err(format!("unsupported filter operator `{op}`")),
    })
}

/// `["match", input, labels, out, ..., fallback]` with boolean outputs.
fn match_test(json: &Json, args: &[Json]) -> Result<Test, String> {
    let bad = || format!("unsupported `match` filter `{json}`");
    let (input, rest) = args.split_first().ok_or_else(bad)?;
    let (fallback, arms) = rest.split_last().ok_or_else(bad)?;
    let input = self::input(input)?;
    let fallback = fallback.as_bool().ok_or_else(bad)?;
    let mut any = Vec::new();
    for arm in arms.chunks(2) {
        let [labels, out] = arm else {
            return Err(bad());
        };
        let labels = match labels {
            Json::Array(values) => values.clone(),
            value => vec![value.clone()],
        };
        if out.as_bool().ok_or_else(bad)? != fallback {
            any.push(Test::OneOf(input.clone(), labels));
        }
    }
    let matched = Test::Any(any);
    Ok(if fallback {
        Test::Not(Box::new(matched))
    } else {
        matched
    })
}

/// A filter's input: a legacy key (`"class"`, `"$type"`) or an expression
/// (`["get", "class"]`, `["geometry-type"]`).
fn input(json: &Json) -> Result<Input, String> {
    match json {
        Json::String(key) if key == "$type" => Ok(Input::GeometryType),
        Json::String(key) => Ok(Input::Property(key.clone())),
        Json::Array(items) => match items.as_slice() {
            [Json::String(op)] if op == "geometry-type" => Ok(Input::GeometryType),
            [Json::String(op), Json::String(key)] if op == "get" => {
                Ok(Input::Property(key.clone()))
            }
            _ => Err(format!("unsupported filter input `{json}`")),
        },
        _ => Err(format!("unsupported filter input `{json}`")),
    }
}

/// A literal operand (`["literal", v]` unwrapped).
fn literal(json: &Json) -> Result<Json, String> {
    match json {
        Json::Array(items) => match items.as_slice() {
            [Json::String(op), value] if op == "literal" => Ok(value.clone()),
            _ => Err(format!("unsupported filter value `{json}`")),
        },
        Json::Object(_) => Err(format!("unsupported filter value `{json}`")),
        value => Ok(value.clone()),
    }
}

fn eval(test: &Test, layer: &Layer, feature: &Feature) -> bool {
    match test {
        Test::Const(b) => *b,
        Test::All(tests) => tests.iter().all(|t| eval(t, layer, feature)),
        Test::Any(tests) => tests.iter().any(|t| eval(t, layer, feature)),
        Test::Not(inner) => !eval(inner, layer, feature),
        Test::Has(key) => layer.property(feature, key).is_some(),
        Test::OneOf(input, values) => {
            let value = read(input, layer, feature);
            values.iter().any(|v| equal(value.as_ref(), v))
        }
        Test::Compare(input, op, expected) => {
            let value = read(input, layer, feature);
            match op {
                Op::Eq => equal(value.as_ref(), expected),
                Op::Ne => !equal(value.as_ref(), expected),
                _ => {
                    let ordering = match (value.as_ref(), expected) {
                        (Some(Value::String(a)), Json::String(b)) => Some(a.as_str().cmp(b)),
                        (Some(v), Json::Number(b)) => v
                            .as_f64()
                            .zip(b.as_f64())
                            .and_then(|(a, b)| a.partial_cmp(&b)),
                        _ => None,
                    };
                    ordering.is_some_and(|o| match op {
                        Op::Lt => o.is_lt(),
                        Op::Le => o.is_le(),
                        Op::Gt => o.is_gt(),
                        _ => o.is_ge(),
                    })
                }
            }
        }
    }
}

/// A feature's value for a filter input.
fn read(input: &Input, layer: &Layer, feature: &Feature) -> Option<Value> {
    match input {
        Input::Property(key) => layer.property(feature, key).cloned(),
        Input::GeometryType => Some(Value::String(
            match feature.geom_type {
                GeomType::Point => "Point",
                GeomType::LineString => "LineString",
                GeomType::Polygon => "Polygon",
                GeomType::Unknown => "Unknown",
            }
            .to_string(),
        )),
    }
}

fn equal(value: Option<&Value>, expected: &Json) -> bool {
    match (value, expected) {
        (None, Json::Null) => true,
        (Some(Value::String(a)), Json::String(b)) => a == b,
        (Some(Value::Bool(a)), Json::Bool(b)) => a == b,
        (Some(v), Json::Number(b)) => v.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a == b),
        _ => false,
    }
}

/// A number that varies with zoom, from a constant, an `interpolate` or
/// `step` expression on `["zoom"]`, or legacy `{"stops": ..., "base": ...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// `(zoom, value)` pairs in increasing zoom order.
    stops: Vec<(f64, f64)>,
    /// Exponential base between stops (`1` is linear).
    base: f64,
    /// Hold each stop's value until the next instead of interpolating.
    step: bool,
}

impl Curve {
    /// A value that doesn't change with zoom.
    pub fn constant(value: f64) -> Self {
        Self {
            stops: vec![(0.0, value)],
            base: 1.0,
            step: false,
        }
    }

    /// Parse a number or a zoom curve of numbers.
    pub fn parse(json: &Json) -> Result<Self, String> {
        if let Some(v) = json.as_f64() {
            return Ok(Self::constant(v));
        }
        let bad = || format!("unsupported value `{json}` (need a number or a zoom curve)");
        if let Some(object) = json.as_object() {
            let zoom_function = !object.contains_key("property")
                && object.get("type").is_none_or(|t| t == "exponential");
            if !zoom_function {
                return Err(bad());
            }
            let stops = object
                .get("stops")
                .and_then(|s| s.as_array())
                .ok_or_else(bad)?
                .iter()
                .map(|s| {
                    let pair = s.as_array().filter(|p| p.len() == 2)?;
                    Some((pair[0].as_f64()?, pair[1].as_f64()?))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(bad)?;
            let base = object.get("base").and_then(|b| b.as_f64()).unwrap_or(1.0);
            return Self::new(stops, base, false).ok_or_else(bad);
        }
        let items = json.as_array().ok_or_else(bad)?;
        let zoom = |v: &Json| v.as_array().is_some_and(|a| a == &[Json::from("zoom")]);
        match items.first().and_then(|o| o.as_str()) {
            Some("interpolate") if items.len() >= 5 && zoom(&items[2]) => {
                let base = match items[1].as_array().map(|a| a.as_slice()) {
                    // MapLibre ignores anything after `linear`.
                    Some([Json::String(kind), ..]) if kind == "linear" => 1.0,
                    Some([Json::String(kind), b]) if kind == "exponential" => {
                        b.as_f64().ok_or_else(bad)?
                    }
                    _ => return Err(bad()),
                };
                let stops = pairs(&items[3..]).ok_or_else(bad)?;
                Self::new(stops, base, false).ok_or_else(bad)
            }
            Some("step") if items.len() >= 3 && zoom(&items[1]) => {
                let first = items[2].as_f64().ok_or_else(bad)?;
                let mut stops = vec![(f64::NEG_INFINITY, first)];
                stops.extend(pairs(&items[3..]).ok_or_else(bad)?);
                Self::new(stops, 1.0, true).ok_or_else(bad)
            }
            _ => Err(bad()),
        }
    }

    /// Parse an on/off property: `true`, `false`, a `step` expression on
    /// `["zoom"]` of them, or legacy `{"stops": [[zoom, bool], ...]}` (which
    /// MapLibre steps too), as a curve of `1` (on) and `0` (off).
    pub fn parse_switch(json: &Json) -> Result<Self, String> {
        let bad = || format!("unsupported value `{json}` (need true, false, or a zoom step)");
        let number = |v: &Json| match v {
            Json::Bool(b) => Json::from(u8::from(*b)),
            other => other.clone(),
        };
        let legacy = |object: &serde_json::Map<String, Json>| -> Option<Json> {
            let stops = object.get("stops")?.as_array()?;
            let (first, rest) = stops.split_first()?;
            let mut items = vec![
                Json::from("step"),
                serde_json::json!(["zoom"]),
                number(first.get(1)?),
            ];
            for stop in rest {
                items.extend([stop.get(0)?.clone(), number(stop.get(1)?)]);
            }
            Some(Json::Array(items))
        };
        let numeric = match json {
            Json::Bool(_) => number(json),
            Json::Array(items) if items.first() == Some(&Json::from("step")) => {
                Json::Array(items.iter().map(number).collect())
            }
            Json::Object(object) => legacy(object).ok_or_else(bad)?,
            _ => return Err(bad()),
        };
        let curve = Self::parse(&numeric).map_err(|_| bad())?;
        if curve.values().all(|v| v == 0.0 || v == 1.0) {
            Ok(curve)
        } else {
            Err(bad())
        }
    }

    /// The zoom from which a switch (see [`Curve::parse_switch`]) stays on:
    /// minus infinity when it is always on, `None` when it is ever off past
    /// its last change.
    pub fn on_from(&self) -> Option<f64> {
        let last_off = self.stops.iter().rposition(|s| s.1 < 0.5);
        match last_off {
            None => Some(f64::NEG_INFINITY),
            Some(i) => self.stops.get(i + 1).map(|s| s.0),
        }
    }

    /// The switch as JSON: `true`/`false` when it never changes, else a
    /// `step` expression of them (see [`Curve::parse_switch`]).
    pub fn switch_json(&self) -> Json {
        let on = |v: f64| Json::Bool(v >= 0.5);
        if self.stops.len() == 1 {
            return on(self.stops[0].1);
        }
        let mut items = vec![
            Json::from("step"),
            serde_json::json!(["zoom"]),
            on(self.stops[0].1),
        ];
        for &(z, v) in &self.stops[1..] {
            items.extend([Json::from(z), on(v)]);
        }
        Json::Array(items)
    }

    fn new(stops: Vec<(f64, f64)>, base: f64, step: bool) -> Option<Self> {
        let ordered = stops.windows(2).all(|w| w[0].0 < w[1].0);
        (!stops.is_empty() && ordered && base > 0.0).then_some(Self { stops, base, step })
    }

    /// The same curve with `f` applied to every value.
    pub fn map(mut self, f: impl Fn(f64) -> f64) -> Self {
        for stop in &mut self.stops {
            stop.1 = f(stop.1);
        }
        self
    }

    /// The values at the stops.
    pub fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.stops.iter().map(|s| s.1)
    }

    /// The value at `zoom` (clamped to the first and last stops).
    pub fn at(&self, zoom: f64) -> f64 {
        let i = self.stops.partition_point(|s| s.0 <= zoom);
        if i == 0 {
            return self.stops[0].1;
        }
        let (z0, v0) = self.stops[i - 1];
        let Some(&(z1, v1)) = self.stops.get(i).filter(|_| !self.step) else {
            return v0;
        };
        let t = if (self.base - 1.0).abs() < 1e-9 {
            (zoom - z0) / (z1 - z0)
        } else {
            (self.base.powf(zoom - z0) - 1.0) / (self.base.powf(z1 - z0) - 1.0)
        };
        v0 + (v1 - v0) * t
    }

    /// The curve as legacy `{"stops": ..., "base": ...}` JSON (a `step`
    /// expression for stepped curves). Even a constant stays a curve, which
    /// a style's `width` reads differently from a plain number.
    pub fn to_json(&self) -> Json {
        if self.step {
            let mut items = vec![
                Json::from("step"),
                serde_json::json!(["zoom"]),
                Json::from(self.stops[0].1),
            ];
            for &(z, v) in &self.stops[1..] {
                items.extend([Json::from(z), Json::from(v)]);
            }
            return Json::Array(items);
        }
        serde_json::json!({
            "stops": self.stops.iter().map(|&(z, v)| [z, v]).collect::<Vec<_>>(),
            "base": self.base,
        })
    }
}

/// `[z0, v0, z1, v1, ...]` as numeric pairs.
fn pairs(items: &[Json]) -> Option<Vec<(f64, f64)>> {
    if !items.len().is_multiple_of(2) {
        return None;
    }
    items
        .chunks(2)
        .map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::mvt::TestPoint;
    use serde_json::json;

    fn layer() -> Layer {
        let points: Vec<TestPoint> = vec![
            (
                (0, 0),
                vec![
                    ("class", Value::String("primary".into())),
                    ("brunnel", Value::String("bridge".into())),
                    ("admin_level", Value::Int(4)),
                ],
            ),
            ((0, 0), vec![("class", Value::String("minor".into()))]),
        ];
        Layer::test_points("transportation", &points)
    }

    fn passing(filter: Json) -> Vec<usize> {
        let layer = layer();
        let filter = Filter::parse(&filter).unwrap();
        (0..layer.features.len())
            .filter(|&i| filter.accepts(&layer, &layer.features[i]))
            .collect()
    }

    #[test]
    fn filters_in_both_syntaxes() {
        assert_eq!(passing(json!(["==", "class", "primary"])), [0]);
        assert_eq!(passing(json!(["==", ["get", "class"], "minor"])), [1]);
        assert_eq!(passing(json!(["in", "class", "minor", "primary"])), [0, 1]);
        assert_eq!(passing(json!(["!in", "class", "minor"])), [0]);
        assert_eq!(passing(json!(["has", "brunnel"])), [0]);
        assert_eq!(passing(json!(["!has", "brunnel"])), [1]);
        assert_eq!(passing(json!(["!=", ["get", "brunnel"], "bridge"])), [1]);
        assert_eq!(passing(json!(["<=", ["get", "admin_level"], 4])), [0]);
        assert_eq!(passing(json!(["==", "$type", "Point"])), [0, 1]);
        assert_eq!(
            passing(json!(["==", ["geometry-type"], "Polygon"])),
            Vec::<usize>::new()
        );
        assert_eq!(
            passing(json!([
                "all",
                ["match", ["get", "class"], ["primary", "trunk"], true, false],
                ["!", ["==", ["get", "brunnel"], "tunnel"]]
            ])),
            [0]
        );
        assert_eq!(
            passing(json!(["match", ["get", "class"], "minor", false, true])),
            [0]
        );
        assert_eq!(
            passing(json!([
                "in",
                ["get", "class"],
                ["literal", ["minor", "service"]]
            ])),
            [1]
        );
        assert!(Filter::parse(&json!(["within", {}])).is_err());
        assert!(Filter::parse(&json!(["==", ["get", ["concat", "a"]], 1])).is_err());
    }

    #[test]
    fn curves_follow_maplibre() {
        let linear =
            Curve::parse(&json!(["interpolate", ["linear"], ["zoom"], 10, 1, 20, 11])).unwrap();
        assert_eq!(linear.at(5.0), 1.0);
        assert_eq!(linear.at(15.0), 6.0);
        assert_eq!(linear.at(25.0), 11.0);
        let exp = Curve::parse(&json!({"stops": [[10, 0], [12, 3]], "base": 2})).unwrap();
        assert!(
            (exp.at(11.0) - 1.0).abs() < 1e-9,
            "2^1 - 1 over 2^2 - 1 of 3"
        );
        let step = Curve::parse(&json!(["step", ["zoom"], 1, 12, 2, 14, 3])).unwrap();
        assert_eq!(
            (step.at(11.0), step.at(12.0), step.at(13.9), step.at(15.0)),
            (1.0, 2.0, 2.0, 3.0)
        );
        for curve in [linear, exp, step, Curve::constant(2.5)] {
            assert_eq!(Curve::parse(&curve.to_json()).unwrap(), curve);
        }
        assert!(Curve::parse(&json!(["get", "width"])).is_err());
        assert!(Curve::parse(&json!({"stops": [[12, 1], [10, 2]]})).is_err());
    }
}
