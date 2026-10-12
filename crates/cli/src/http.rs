//! The one HTTP client every download shares.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::blocking::Client;

/// Identifies this tool to tile servers, as their usage policies ask.
const USER_AGENT: &str = concat!(
    "gpx-motion/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/cbcruk/gpx-motion)"
);

/// The shared client (built on first use), so connections are reused.
pub fn client() -> Result<&'static Client, reqwest::Error> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// GET `url`.
pub fn fetch_bytes(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(client()?
        .get(url)
        .send()?
        .error_for_status()?
        .bytes()?
        .to_vec())
}
