//! A person's day: the calendar date where they are.

use anyhow::{anyhow, Result};
use chrono::Utc;
use chrono_tz::Tz;

pub fn zone(name: &str) -> Result<Tz> {
    name.parse()
        .map_err(|_| anyhow!("`{name}` is not an IANA time zone, such as Africa/Johannesburg"))
}

/// Today in that zone, as YYYY-MM-DD.
pub fn today(zone: Tz) -> String {
    Utc::now()
        .with_timezone(&zone)
        .format("%Y-%m-%d")
        .to_string()
}

/// Their date and time as the prompt says it: "Friday 23 October 2026, 17:42 (Africa/Johannesburg)".
pub fn now(zone: Tz) -> String {
    format!(
        "{} ({})",
        Utc::now()
            .with_timezone(&zone)
            .format("%A %-d %B %Y, %H:%M"),
        zone.name()
    )
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
