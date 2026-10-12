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
    /// A coordinate or elevation isn't a number (the offending text).
    InvalidNumber(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Xml(e) => write!(f, "XML error: {e}"),
            ParseError::MissingCoordinate => write!(f, "trkpt missing lat/lon attribute"),
            ParseError::InvalidNumber(s) => write!(f, "invalid number: {s}"),
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
/// `lon`, or a coordinate or elevation isn't a number.
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
                            pt.ele = text.parse::<f64>().ok();
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
    fn missing_coordinate_is_error() {
        let xml = r#"<gpx><trk><trkseg><trkpt lat="1.0"/></trkseg></trk></gpx>"#;
        assert!(matches!(parse(xml), Err(ParseError::MissingCoordinate)));
    }
}
