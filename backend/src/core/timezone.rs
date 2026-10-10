//! Kronn's global timezone (KT-1103): the zone a cron or watch trigger without
//! its own `timezone` is read in, and the default of `{{time.now}}` templates.
//!
//! `server.timezone` in config.toml wins; otherwise the machine's zone is
//! detected at boot (`TZ`, then the OS setting), with UTC as the fallback. A
//! Docker container usually reports UTC unless the compose file passes `TZ`.

use chrono_tz::Tz;
use std::sync::{OnceLock, RwLock};

static CURRENT: OnceLock<RwLock<Tz>> = OnceLock::new();

// UTC until boot arms it, so tests never depend on the machine's zone.
fn cell() -> &'static RwLock<Tz> {
    CURRENT.get_or_init(|| RwLock::new(Tz::UTC))
}

/// The zone in effect for this process.
pub fn current() -> Tz {
    *cell().read().unwrap_or_else(|e| e.into_inner())
}

/// Arms the process zone from the configured value (or the machine's zone).
/// Boot goes through `crate::load_startup_config`; Settings calls it on save.
pub fn apply(configured: Option<&str>) -> Tz {
    apply_with(configured, detect_machine_timezone())
}

/// As [`apply`], with the machine zone already detected.
pub fn apply_with(configured: Option<&str>, detected: Tz) -> Tz {
    let tz = resolve(configured, detected);
    *cell().write().unwrap_or_else(|e| e.into_inner()) = tz;
    tz
}

/// The configured zone when it is a valid IANA name, else `detected`.
pub fn resolve(configured: Option<&str>, detected: Tz) -> Tz {
    configured
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(detected)
}

/// Parses an IANA name, with the message the API returns on a typo.
pub fn parse(name: &str) -> Result<Tz, String> {
    name.trim()
        .parse::<Tz>()
        .map_err(|_| format!("Unknown timezone `{name}`: use an IANA name such as Europe/Paris."))
}

/// The machine's zone: `TZ`, then the OS setting, then UTC.
pub fn detect_machine_timezone() -> Tz {
    let env = crate::core::child_env::var("TZ").ok();
    let os = iana_time_zone::get_timezone().ok();
    detect_from(env.as_deref(), os.as_deref())
}

/// `TZ` may be `Europe/Paris`, `:Europe/Paris` or a zoneinfo path.
pub fn detect_from(tz_env: Option<&str>, os_zone: Option<&str>) -> Tz {
    let from_env = tz_env.map(str::trim).and_then(|raw| {
        let name = raw.trim_start_matches(':');
        let name = name.rsplit_once("zoneinfo/").map_or(name, |(_, zone)| zone);
        name.parse::<Tz>().ok()
    });
    from_env
        .or_else(|| os_zone.and_then(|name| name.trim().parse::<Tz>().ok()))
        .unwrap_or(Tz::UTC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_prefers_tz_then_the_os_then_utc() {
        assert_eq!(
            detect_from(Some("Europe/Paris"), Some("Asia/Tokyo")),
            Tz::Europe__Paris
        );
        assert_eq!(detect_from(Some(":Europe/Paris"), None), Tz::Europe__Paris);
        assert_eq!(
            detect_from(Some("/usr/share/zoneinfo/America/New_York"), None),
            Tz::America__New_York
        );
        // An empty or unknown TZ (compose `TZ=${TZ:-}`) falls through to the OS.
        assert_eq!(detect_from(Some(""), Some("Asia/Tokyo")), Tz::Asia__Tokyo);
        assert_eq!(
            detect_from(Some("CEST-2"), Some("Asia/Tokyo")),
            Tz::Asia__Tokyo
        );
        assert_eq!(detect_from(None, Some("nonsense")), Tz::UTC);
        assert_eq!(detect_from(None, None), Tz::UTC);
    }

    #[test]
    fn a_configured_zone_wins_and_an_empty_one_means_the_machine() {
        assert_eq!(
            resolve(Some("Asia/Tokyo"), Tz::Europe__Paris),
            Tz::Asia__Tokyo
        );
        assert_eq!(resolve(Some("  "), Tz::Europe__Paris), Tz::Europe__Paris);
        assert_eq!(resolve(None, Tz::Europe__Paris), Tz::Europe__Paris);
        assert_eq!(resolve(Some("Mars/Olympus"), Tz::UTC), Tz::UTC);
        assert!(parse("Mars/Olympus").unwrap_err().contains("IANA"));
    }
}
