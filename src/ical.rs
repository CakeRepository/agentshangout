//! Renders iCalendar (RFC 5545) feeds. Every field here can come from someone else's bot, so
//! escaping is what stops a crafted title from adding properties to a person's calendar.

use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

use crate::db::{Event, FeedEvent};

const STAMP: &[FormatItem<'static>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

fn stamp(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .format(STAMP)
        .unwrap_or_default()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

/// Writes one content line, folded at 75 octets without splitting a UTF-8 character.
fn push_line(out: &mut String, line: &str) {
    let mut width = 0;
    for ch in line.chars() {
        if width + ch.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(ch);
        width += ch.len_utf8();
    }
    out.push_str("\r\n");
}

fn push_event(out: &mut String, event: &Event, status: &str, description: &str, now: i64) {
    push_line(out, "BEGIN:VEVENT");
    push_line(out, &format!("UID:{}@agenthangout", event.id));
    push_line(out, &format!("DTSTAMP:{}", stamp(now)));
    push_line(out, &format!("LAST-MODIFIED:{}", stamp(event.updated_at)));
    push_line(out, &format!("SEQUENCE:{}", event.sequence));
    push_line(out, &format!("DTSTART:{}", stamp(event.starts_at)));
    push_line(out, &format!("DTEND:{}", stamp(event.ends_at)));
    push_line(out, &format!("SUMMARY:{}", escape(&event.title)));
    if !event.location.is_empty() {
        push_line(out, &format!("LOCATION:{}", escape(&event.location)));
    }
    if !description.is_empty() {
        push_line(out, &format!("DESCRIPTION:{}", escape(description)));
    }
    push_line(out, &format!("STATUS:{status}"));
    push_line(out, "END:VEVENT");
}

fn header(out: &mut String, name: &str) {
    push_line(out, "BEGIN:VCALENDAR");
    push_line(out, "VERSION:2.0");
    push_line(out, "PRODID:-//Agents Hangout//EN");
    push_line(out, "CALSCALE:GREGORIAN");
    push_line(out, "METHOD:PUBLISH");
    push_line(out, &format!("X-WR-CALNAME:{}", escape(name)));
    push_line(out, "REFRESH-INTERVAL;VALUE=DURATION:PT1H");
    push_line(out, "X-PUBLISHED-TTL:PT1H");
}

/// Tentative until the group confirms it and this person said a firm yes.
fn feed_status(event: &Event, response: &str) -> &'static str {
    match event.status.as_str() {
        "cancelled" => "CANCELLED",
        "confirmed" if response == "yes" => "CONFIRMED",
        _ => "TENTATIVE",
    }
}

pub fn feed(person_name: &str, events: &[FeedEvent], now: i64) -> String {
    let mut out = String::new();
    header(&mut out, &format!("{person_name}'s hangouts"));
    for item in events {
        let mut description = format!("From {} on Agents Hangout.", item.hangout_name);
        if !item.event.notes.is_empty() {
            description = format!("{}\n\n{description}", item.event.notes);
        }
        let status = feed_status(&item.event, &item.response);
        push_event(&mut out, &item.event, status, &description, now);
    }
    push_line(&mut out, "END:VCALENDAR");
    out
}

/// A single event, for the one-click "add to calendar" download.
pub fn single(event: &Event, now: i64) -> String {
    let mut out = String::new();
    header(&mut out, &event.title);
    let status = feed_status(event, "yes");
    push_event(&mut out, event, status, &event.notes, now);
    push_line(&mut out, "END:VCALENDAR");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(title: &str) -> Event {
        Event {
            id: "evt1".into(),
            hangout_id: "h1".into(),
            title: title.into(),
            location: String::new(),
            notes: String::new(),
            starts_at: 1_793_750_400,
            ends_at: 1_793_757_600,
            status: "confirmed".into(),
            min_yes: 2,
            sequence: 1,
            proposed_by: "p1".into(),
            proposer: "Sam".into(),
            updated_at: 1_793_750_000,
            created_at: 1_793_750_000,
            rsvps: Vec::new(),
        }
    }

    #[test]
    fn crafted_title_cannot_add_properties() {
        let ics = single(&event("Tacos\r\nATTENDEE:mailto:evil@example.com"), 0);
        assert!(!ics.contains("\r\nATTENDEE"), "{ics}");
        assert!(ics.contains("SUMMARY:Tacos\\nATTENDEE:mailto:evil@example.com"));
    }

    #[test]
    fn escapes_separators() {
        assert_eq!(escape("a,b;c\\d"), "a\\,b\\;c\\\\d");
    }

    #[test]
    fn folds_long_lines_on_character_boundaries() {
        let ics = single(&event(&"🌮".repeat(60)), 0);
        for line in ics.split("\r\n") {
            assert!(line.len() <= 75, "{} octets: {line}", line.len());
        }
        let unfolded = ics.replace("\r\n ", "");
        assert!(unfolded.contains(&format!("SUMMARY:{}", "🌮".repeat(60))));
    }

    #[test]
    fn times_are_utc() {
        let ics = single(&event("x"), 0);
        assert!(ics.contains("DTSTART:20261104T"), "{ics}");
        assert!(ics.contains("Z\r\n"));
    }

    #[test]
    fn maybe_stays_tentative_on_a_confirmed_event() {
        assert_eq!(feed_status(&event("x"), "maybe"), "TENTATIVE");
        assert_eq!(feed_status(&event("x"), "yes"), "CONFIRMED");
    }
}
