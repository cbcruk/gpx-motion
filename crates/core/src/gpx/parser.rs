//! A streaming GPX parser that flattens every track segment into one list
//! of points.

use std::fmt;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::model::TrackPoint;

/// Why a GPX document couldn't be read.
#[derive(Debug)]
pub enum ParseError {
    /// The document isn't well-formed XML.
    Xml(quick_xml::Error),
    /// A `<trkpt>` lacks its `lat` or `lon` attribute.
    MissingCoordinate,
    /// A coordinate isn't a number (the offending text).
    InvalidNumber(String),
    /// The `point`-th `<trkpt>` (counting from 1) has a latitude outside
    /// `-90..=90` or a longitude outside `-180..=180`, or one that isn't
    /// finite (`NaN`, `inf`).
    CoordinateOutOfRange {
        /// Which `<trkpt>` in the file, counting from 1.
        point: usize,
        /// Its latitude as read.
        lat: f64,
        /// Its longitude as read.
        lon: f64,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Xml(e) => write!(f, "XML error: {e}"),
            ParseError::MissingCoordinate => write!(f, "trkpt missing lat/lon attribute"),
            ParseError::InvalidNumber(s) => write!(f, "invalid number: {s}"),
            ParseError::CoordinateOutOfRange { point, lat, lon } => write!(
                f,
                "trkpt {point}: lat {lat} / lon {lon} is not a coordinate \
                 (lat must be within -90..=90, lon within -180..=180)"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<quick_xml::Error> for ParseError {
    fn from(e: quick_xml::Error) -> Self {
        ParseError::Xml(e)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    None,
    Ele,
    Time,
}

/// Parse a GPX document into a flat list of track points, in file order.
///
/// All `<trkpt>` across every `<trkseg>`/`<trk>` are concatenated; segment
/// boundaries are not preserved, so the track draws as one polyline.
///
/// # Errors
///
/// A [`ParseError`] when `xml` is malformed, a `<trkpt>` lacks `lat` or
/// `lon`, a coordinate isn't a number, or a coordinate is out of range or
/// not finite. An elevation that isn't a finite number is dropped (the
/// point has none) rather than refused.
pub fn parse(xml: &str) -> Result<Vec<TrackPoint>, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut points = Vec::new();
    let mut current: Option<TrackPoint> = None;
    let mut field = Field::None;

    loop {
        let event = reader.read_event()?;
        let empty_trkpt = matches!(&event, Event::Empty(e) if e.local_name().as_ref() == b"trkpt");
        match event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"trkpt" => {
                let mut lat: Option<f64> = None;
                let mut lon: Option<f64> = None;
                for attr in e.attributes() {
                    let attr = attr.map_err(quick_xml::Error::from)?;
                    let value = || -> Result<f64, ParseError> {
                        let raw = attr.unescape_value()?;
                        raw.parse::<f64>()
                            .map_err(|_| ParseError::InvalidNumber(raw.into_owned()))
                    };
                    match attr.key.local_name().as_ref() {
                        b"lat" => lat = Some(value()?),
                        b"lon" => lon = Some(value()?),
                        _ => {}
                    }
                }
                let (lat, lon) = lat.zip(lon).ok_or(ParseError::MissingCoordinate)?;
                if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
                    return Err(ParseError::CoordinateOutOfRange {
                        point: points.len() + 1,
                        lat,
                        lon,
                    });
                }
                let point = TrackPoint::new(lat, lon);
                // A self-closing <trkpt/> has no matching End event to flush on.
                if empty_trkpt {
                    points.push(point);
                } else {
                    current = Some(point);
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                b"ele" => field = Field::Ele,
                b"time" => field = Field::Time,
                _ => {}
            },
            Event::Text(e) if field != Field::None => {
                if let Some(pt) = current.as_mut() {
                    let text = e.unescape()?;
                    match field {
                        Field::Ele => {
                            pt.ele = text.parse::<f64>().ok().filter(|e| e.is_finite());
                        }
                        Field::Time => pt.time = Some(text.into_owned()),
                        Field::None => {}
                    }
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"trkpt" => {
                    if let Some(pt) = current.take() {
                        points.push(pt);
                    }
                }
                b"ele" | b"time" => field = Field::None,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1">
  <trk><trkseg>
    <trkpt lat="37.5665" lon="126.9780">
      <ele>38.0</ele>
      <time>2024-01-01T00:00:00Z</time>
    </trkpt>
    <trkpt lat="37.5670" lon="126.9785"/>
  </trkseg></trk>
</gpx>"#;

    #[test]
    fn parses_points_in_order() {
        let pts = parse(SAMPLE).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].lat, 37.5665);
        assert_eq!(pts[0].lon, 126.9780);
        assert_eq!(pts[1].lat, 37.5670);
    }

    #[test]
    fn captures_optional_ele_and_time() {
        let pts = parse(SAMPLE).unwrap();
        assert_eq!(pts[0].ele, Some(38.0));
        assert_eq!(pts[0].time.as_deref(), Some("2024-01-01T00:00:00Z"));
        assert_eq!(pts[1].ele, None);
        assert_eq!(pts[1].time, None);
    }

    #[test]
    fn non_finite_or_out_of_range_coordinates_are_refused() {
        let trkpt = |lat: &str, lon: &str| {
            format!(
                r#"<gpx><trk><trkseg><trkpt lat="1" lon="2"/><trkpt lat="{lat}" lon="{lon}"/></trkseg></trk></gpx>"#
            )
        };
        for (lat, lon) in [
            ("NaN", "126.9"),
            ("37.5", "nan"),
            ("inf", "126.9"),
            ("37.5", "-infinity"),
            ("90.5", "126.9"),
            ("-91", "126.9"),
            ("37.5", "180.1"),
            ("37.5", "-200"),
        ] {
            let err = parse(&trkpt(lat, lon)).unwrap_err();
            assert!(
                matches!(err, ParseError::CoordinateOutOfRange { point: 2, .. }),
                "{lat} {lon}: {err}"
            );
            assert!(err.to_string().starts_with("trkpt 2: "), "{err}");
        }
        // The ends of the ranges, signs and exponents are coordinates.
        for (lat, lon) in [("90", "180"), ("-90", "-180"), ("+3.75e1", "1.269e2")] {
            assert!(parse(&trkpt(lat, lon)).is_ok(), "{lat} {lon}");
        }
    }

    #[test]
    fn the_refused_point_is_counted_across_segments_and_tracks() {
        let xml = r#"<gpx>
            <trk><trkseg><trkpt lat="1" lon="2"/><trkpt lat="1" lon="2"></trkpt></trkseg>
                 <trkseg><trkpt lat="1" lon="2"/></trkseg></trk>
            <trk><trkseg><trkpt lat="1" lon="2"/><trkpt lat="1" lon="999"/></trkseg></trk>
        </gpx>"#;
        assert!(matches!(
            parse(xml),
            Err(ParseError::CoordinateOutOfRange { point: 5, .. })
        ));
    }

    #[test]
    fn non_finite_elevations_are_dropped() {
        let xml = r#"<gpx><trk><trkseg>
            <trkpt lat="1" lon="2"><ele>NaN</ele></trkpt>
            <trkpt lat="1" lon="2"><ele>inf</ele></trkpt>
            <trkpt lat="1" lon="2"><ele>-12.5</ele></trkpt>
        </trkseg></trk></gpx>"#;
        let eles: Vec<_> = parse(xml).unwrap().iter().map(|p| p.ele).collect();
        assert_eq!(eles, [None, None, Some(-12.5)]);
    }

    #[test]
    fn missing_coordinate_is_error() {
        let xml = r#"<gpx><trk><trkseg><trkpt lat="1.0"/></trkseg></trk></gpx>"#;
        assert!(matches!(parse(xml), Err(ParseError::MissingCoordinate)));
    }
}
