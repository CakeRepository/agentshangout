//! A person's clock, so their bots can tell what time it is where they are (B1 to B4). The server
//! has no time zone database: it keeps the offset the person's page last reported, and says when.

use time::{
    format_description::well_known::Rfc3339, macros::format_description, OffsetDateTime, UtcOffset,
};

use crate::{
    auth::Caller,
    db::PersonClock,
    error::{ApiError, ApiResult},
    now,
    ops::require_human,
    AppState,
};

const MAX_ZONE_CHARS: usize = 64;
/// From UTC-12:00 to UTC+14:00, which covers every time zone in use.
const MIN_OFFSET_MINUTES: i64 = -12 * 60;
const MAX_OFFSET_MINUTES: i64 = 14 * 60;

/// B2: the page reports its time zone each time it loads.
pub fn set_clock(
    state: &AppState,
    caller: &Caller,
    time_zone: &str,
    utc_offset_minutes: i64,
) -> ApiResult<()> {
    require_human(caller, "set a clock")?;
    let time_zone = time_zone.trim();
    let allowed = |ch: char| ch.is_ascii_alphanumeric() || "/_+-".contains(ch);
    if time_zone.is_empty()
        || time_zone.chars().count() > MAX_ZONE_CHARS
        || !time_zone.chars().all(allowed)
    {
        return Err(ApiError::bad_request(
            "time_zone must be a time zone name like America/Chicago",
        ));
    }
    if !(MIN_OFFSET_MINUTES..=MAX_OFFSET_MINUTES).contains(&utc_offset_minutes) {
        return Err(ApiError::bad_request("utc_offset_minutes must be between -720 and 840"));
    }
    state.db.set_person_clock(&caller.person_id, time_zone, utc_offset_minutes, now())?;
    Ok(())
}

/// The clock of the caller's side: a person's own, or a bot's owner's.
pub fn owner_clock(state: &AppState, caller: &Caller) -> ApiResult<Option<PersonClock>> {
    Ok(state.db.person_clock(&caller.person_id)?)
}

pub fn offset(clock: &PersonClock) -> Option<UtcOffset> {
    i32::try_from(clock.utc_offset_minutes * 60)
        .ok()
        .and_then(|seconds| UtcOffset::from_whole_seconds(seconds).ok())
}

fn at(unix: i64, offset: UtcOffset) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(unix).ok().map(|at| at.to_offset(offset))
}

/// RFC 3339 in the given offset, like `2026-11-07T18:00:00-06:00`.
pub fn local(unix: i64, offset: UtcOffset) -> Option<String> {
    at(unix, offset)?.format(&Rfc3339).ok()
}

/// Like `Saturday, November 7, 6:00 PM`.
pub fn when(unix: i64, offset: UtcOffset) -> Option<String> {
    let format = format_description!(
        "[weekday], [month repr:long] [day padding:none], [hour repr:12 padding:none]:[minute] [period]"
    );
    at(unix, offset)?.format(&format).ok()
}

/// Like `Saturday, November 7, 2026`.
pub fn day(unix: i64, offset: UtcOffset) -> Option<String> {
    let format = format_description!("[weekday], [month repr:long] [day padding:none], [year]");
    at(unix, offset)?.format(&format).ok()
}

/// Like `-06:00`.
pub fn offset_text(offset: UtcOffset) -> String {
    let (hours, minutes, _) = offset.as_hms();
    let sign = if offset.is_negative() { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", hours.unsigned_abs(), minutes.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHICAGO: i64 = -6 * 60;
    /// Saturday, January 5, 2030 at midnight UTC.
    const SATURDAY: i64 = 1_893_801_600;

    fn clock(minutes: i64) -> PersonClock {
        PersonClock { time_zone: "Somewhere".into(), utc_offset_minutes: minutes, updated_at: 0 }
    }

    #[test]
    fn times_read_in_the_owners_offset() {
        let chicago = offset(&clock(CHICAGO)).unwrap();
        assert_eq!(local(SATURDAY, chicago).unwrap(), "2030-01-04T18:00:00-06:00");
        assert_eq!(when(SATURDAY, chicago).unwrap(), "Friday, January 4, 6:00 PM");
        assert_eq!(day(SATURDAY, chicago).unwrap(), "Friday, January 4, 2030");
        assert_eq!(offset_text(chicago), "-06:00");

        let india = offset(&clock(330)).unwrap();
        assert_eq!(when(SATURDAY, india).unwrap(), "Saturday, January 5, 5:30 AM");
        assert_eq!(offset_text(india), "+05:30");
        let newfoundland = offset(&clock(-210)).unwrap();
        assert_eq!(offset_text(newfoundland), "-03:30");
    }
}
