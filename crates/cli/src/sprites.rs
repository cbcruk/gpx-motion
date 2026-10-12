//! Average colors of the sprite patterns a MapLibre style fills with.

use anyhow::Result;
use gpx_motion_core::vector::maplibre::PatternColors;
use serde_json::Value as Json;
use tiny_skia::Color;

/// The average color of each of `names` in the sprite of `style` (a
/// MapLibre style), fetched with `fetch`. Patterns that can't be read are
/// left out, with a warning when the sprite itself can't be.
pub fn pattern_colors(
    style: &Json,
    names: &[String],
    fetch: impl Fn(&str) -> Result<Vec<u8>>,
) -> PatternColors {
    let mut colors = PatternColors::new();
    let found = sprites(style);
    if found.is_empty() {
        eprintln!("warning: map style: no sprite at a web URL; pattern layers are skipped");
    }
    for (prefix, url) in found {
        let wanted: Vec<(String, &str)> = names
            .iter()
            .filter_map(|n| Some((n.clone(), n.strip_prefix(&prefix)?)))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let read = || -> Result<PatternColors> {
            let index: Json = serde_json::from_slice(&fetch(&with_suffix(&url, ".json"))?)?;
            let image = fetch(&with_suffix(&url, ".png"))?;
            average(&image, &index, &wanted)
        };
        match read() {
            Ok(found) => colors.extend(found),
            Err(e) => eprintln!("warning: map style: sprite {url}: {e}"),
        }
    }
    colors
}

/// `url` with `suffix` on its path, before any query (as MapLibre adds
/// `.json` and `.png` to sprite URLs carrying keys).
fn with_suffix(url: &str, suffix: &str) -> String {
    match url.split_once('?') {
        Some((path, query)) => format!("{path}{suffix}?{query}"),
        None => format!("{url}{suffix}"),
    }
}

/// Each sprite of `style` as the prefix its pattern names carry and its URL
/// (without `.json`/`.png`): one unprefixed `sprite` URL, or MapLibre's list
/// of `{"id", "url"}` (named `id:name`, except `default`).
fn sprites(style: &Json) -> Vec<(String, String)> {
    let web = |url: &str| url.starts_with("http://") || url.starts_with("https://");
    match style.get("sprite") {
        Some(Json::String(url)) if web(url) => vec![(String::new(), url.clone())],
        Some(Json::Array(list)) => list
            .iter()
            .filter_map(|s| {
                let (id, url) = (s.get("id")?.as_str()?, s.get("url")?.as_str()?);
                let prefix = if id == "default" {
                    String::new()
                } else {
                    format!("{id}:")
                };
                web(url).then(|| (prefix, url.to_string()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The average color of each `(name, key)` pattern: the `key` entry of
/// sprite `index` cut from the PNG `image`, its colors weighted by alpha
/// and its alpha averaged.
fn average(image: &[u8], index: &Json, wanted: &[(String, &str)]) -> Result<PatternColors> {
    let rgba = image::load_from_memory(image)?.to_rgba8();
    let mut colors = PatternColors::new();
    for (name, key) in wanted {
        let Some(entry) = index.get(*key) else {
            continue;
        };
        let field = |k: &str| {
            entry
                .get(k)
                .and_then(Json::as_u64)
                .and_then(|v| u32::try_from(v).ok())
        };
        let (Some(x), Some(y), Some(w), Some(h)) =
            (field("x"), field("y"), field("width"), field("height"))
        else {
            continue;
        };
        let inside =
            |start: u32, size: u32, max: u32| start.checked_add(size).is_some_and(|e| e <= max);
        if w == 0 || h == 0 || !inside(x, w, rgba.width()) || !inside(y, h, rgba.height()) {
            continue;
        }
        let (mut sum, mut weight) = ([0.0f64; 3], 0.0f64);
        for py in y..y + h {
            for px in x..x + w {
                let [r, g, b, a] = rgba.get_pixel(px, py).0;
                let a = a as f64 / 255.0;
                for (s, c) in sum.iter_mut().zip([r, g, b]) {
                    *s += c as f64 / 255.0 * a;
                }
                weight += a;
            }
        }
        let alpha = weight / (w * h) as f64;
        let channel = |s: f64| {
            if weight > 0.0 {
                (s / weight) as f32
            } else {
                0.0
            }
        };
        if let Some(color) = Color::from_rgba(
            channel(sum[0]),
            channel(sum[1]),
            channel(sum[2]),
            alpha as f32,
        ) {
            colors.insert(name.clone(), color);
        }
    }
    Ok(colors)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×2 sprite: a red-and-clear pattern `half` on the left, solid blue
    /// `blue` on the right, as PNG bytes.
    fn sprite() -> Vec<u8> {
        let mut image = image::RgbaImage::new(4, 2);
        for (x, y, px) in image.enumerate_pixels_mut() {
            *px = match (x, y) {
                (0..=1, 0) => image::Rgba([255, 0, 0, 255]),
                (0..=1, _) => image::Rgba([0, 255, 0, 0]),
                _ => image::Rgba([0, 0, 255, 255]),
            };
        }
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    fn index() -> Json {
        serde_json::json!({
            "half": {"x": 0, "y": 0, "width": 2, "height": 2, "pixelRatio": 1},
            "blue": {"x": 2, "y": 0, "width": 2, "height": 2, "pixelRatio": 1}
        })
    }

    #[test]
    fn entries_past_the_image_are_skipped() {
        let index = serde_json::json!({
            "far": {"x": 4294967295u64, "y": 0, "width": 2, "height": 2},
            "wide": {"x": 3, "y": 0, "width": 2, "height": 2}
        });
        let wanted = [("far".to_string(), "far"), ("wide".to_string(), "wide")];
        assert!(average(&sprite(), &index, &wanted).unwrap().is_empty());
    }

    #[test]
    fn patterns_average_their_visible_pixels() {
        let wanted = [("half".to_string(), "half"), ("blue".to_string(), "blue")];
        let colors = average(&sprite(), &index(), &wanted).unwrap();
        let half = colors["half"];
        // Clear pixels don't tint it green; they make it half transparent.
        assert_eq!((half.red(), half.green(), half.alpha()), (1.0, 0.0, 0.5));
        assert_eq!(colors["blue"], Color::from_rgba8(0, 0, 255, 255));
    }

    #[test]
    fn named_sprites_prefix_their_patterns() {
        let style = serde_json::json!({"sprite": [
            {"id": "default", "url": "https://example.com/a"},
            {"id": "extra", "url": "https://example.com/b"}]});
        let names = ["half".to_string(), "extra:blue".to_string()];
        let colors = pattern_colors(&style, &names, |url| match url {
            u if u.ends_with(".json") => Ok(index().to_string().into_bytes()),
            _ => Ok(sprite()),
        });
        assert_eq!(colors.len(), 2);
        assert_eq!(colors["extra:blue"], Color::from_rgba8(0, 0, 255, 255));
        // A query stays after the suffix.
        let keyed = serde_json::json!({"sprite": "https://example.com/s?key=k"});
        let urls = std::cell::RefCell::new(Vec::new());
        pattern_colors(&keyed, &names, |url| {
            urls.borrow_mut().push(url.to_string());
            Err(anyhow::anyhow!("offline"))
        });
        assert_eq!(urls.borrow()[0], "https://example.com/s.json?key=k");
        // A relative or missing sprite is no sprite.
        let local = serde_json::json!({"sprite": "mapbox://sprites/x"});
        assert!(pattern_colors(&local, &names, |_| unreachable!()).is_empty());
    }
}
