//! A person's day: the calendar date where they are.

use anyhow::{anyhow, Result};
use chrono::Utc;
use chrono_tz::Tz;

pub fn zone(name: &str) -> Result<Tz> {
    name.parse()
        .map_err(|_| anyhow!("`{name}` is not an IANA time zone, such as Africa/Johannesburg"))
}

/// One instant where they are, so the day and the time the prompt shows cannot straddle midnight.
pub struct Moment {
    /// Today as YYYY-MM-DD.
    pub day: String,
    /// Their date and time as the prompt says it: "Friday 23 October 2026, 17:42 (Africa/Johannesburg)".
    pub display: String,
}

impl Moment {
    pub fn new(zone: Tz) -> Self {
        let now = Utc::now().with_timezone(&zone);
        Self {
            day: now.format("%Y-%m-%d").to_string(),
            display: format!("{} ({})", now.format("%A %-d %B %Y, %H:%M"), zone.name()),
        }
    }
}

pub fn is_day(value: &str) -> bool {
    value.len() == 10 && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// This machine's time zone: `TZ`, else where /etc/localtime points, else UTC.
pub fn system_zone() -> String {
    std::env::var("TZ")
        .ok()
        .filter(|tz| tz.parse::<Tz>().is_ok())
        .or_else(|| {
            let link = std::fs::read_link("/etc/localtime").ok()?;
            let link = link.to_string_lossy();
            let name = link.split("zoneinfo/").nth(1)?.to_string();
            name.parse::<Tz>().is_ok().then_some(name)
        })
        .unwrap_or_else(|| "UTC".to_string())
}
