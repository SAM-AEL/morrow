use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How often a scheduled task repeats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Recurrence {
    /// One shot.
    Once,
    /// Every day at the given local time.
    Daily { time: NaiveTime },
    /// Monday-Friday at the given local time.
    Weekdays { time: NaiveTime },
    /// A specific weekday (chrono: Mon=1..Sun=7) at the given local time.
    Weekly { weekday: u32, time: NaiveTime },
    /// Every N minutes (minimum 1).
    Interval { minutes: i64 },
}

impl Recurrence {
    /// Next UTC instant strictly after `after`, computed against DEVICE-LOCAL time.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local_after = Local.from_utc_datetime(&after.naive_utc());
        match self {
            Recurrence::Once => None,
            Recurrence::Interval { minutes } => Some(after + Duration::minutes(*minutes.max(&1))),
            Recurrence::Daily { time }
            | Recurrence::Weekdays { time }
            | Recurrence::Weekly { time, .. } => {
                let mut date = local_after.date_naive();
                // candidate same-day
                loop {
                    let cand_local = date.and_time(*time);
                    if let Some(cand) = Local.from_local_datetime(&cand_local).single() {
                        if cand > local_after {
                            match self {
                                Recurrence::Daily { .. } => return Some(cand.with_timezone(&Utc)),
                                Recurrence::Weekdays { .. } => {
                                    if is_weekday(date.weekday().num_days_from_monday()) {
                                        return Some(cand.with_timezone(&Utc));
                                    }
                                }
                                Recurrence::Weekly { weekday, .. } => {
                                    if date.weekday().number_from_monday() == *weekday {
                                        return Some(cand.with_timezone(&Utc));
                                    }
                                }
                                Recurrence::Once | Recurrence::Interval { .. } => unreachable!(),
                            }
                        }
                    }
                    date = date.succ_opt()?;
                }
            }
        }
    }
}

fn is_weekday(days_from_monday: u32) -> bool {
    days_from_monday < 5
}

/// A scheduled task owned by one character.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    /// Prompt executed when due.
    pub task: String,
    /// Human description of the timing, as originally written.
    pub when_text: String,
    #[serde(flatten)]
    pub recurrence: Recurrence,
    pub next_run: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Schedule {
    pub fn new(when_text: &str, task: &str) -> Result<Self> {
        let (recurrence, first_run) = parse_when(when_text)?;
        Ok(Self {
            id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            task: task.to_string(),
            when_text: when_text.to_string(),
            recurrence,
            next_run: first_run,
            created_at: Utc::now(),
            last_run: None,
            enabled: true,
        })
    }

    /// Call after a run: reschedule or disable one-shots.
    pub fn completed(&mut self) {
        self.last_run = Some(Utc::now());
        match self.recurrence.next_after(Utc::now()) {
            Some(next) => self.next_run = next,
            None => self.enabled = false,
        }
    }

    pub fn describe(&self) -> String {
        let next = Local.from_utc_datetime(&self.next_run.naive_utc());
        let rec = match &self.recurrence {
            Recurrence::Once => "once".into(),
            Recurrence::Daily { .. } => "daily".into(),
            Recurrence::Weekdays { .. } => "weekdays".into(),
            Recurrence::Weekly { weekday, .. } => {
                format!("every {}", weekday_name(*weekday))
            }
            Recurrence::Interval { minutes } => format!("every {minutes} min"),
        };
        format!(
            "[{}] {} · {} · next {}{}",
            self.id,
            self.when_text,
            rec,
            next.format("%a %Y-%m-%d %H:%M %Z"),
            if self.enabled { "" } else { " · DONE" }
        )
    }
}

fn weekday_name(n: u32) -> &'static str {
    match n {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        7 => "Sunday",
        _ => "?",
    }
}

/// Parse a human "when" expression against DEVICE-LOCAL time.
/// Supported forms (case-insensitive, flexible whitespace):
///   12am | midnight | noon | 9:30pm | 07:15
///   today at 5pm · tonight at 11pm · tomorrow at 9am
///   every day at 6pm · daily at noon · weekdays at 08:00
///   every monday at 9am · every fri 17:00
///   hourly
///   in 30 minutes · in 2 hours · in 3 days · in 45 seconds
pub fn parse_when(input: &str) -> Result<(Recurrence, DateTime<Utc>)> {
    let s = input.trim().to_lowercase();
    let s = s.trim_start_matches(['"', '\'']);
    let s = s.trim();

    let now_local = Local::now();

    // --- relative: "in N units" ---
    if let Some(rest) = s.strip_prefix("in ") {
        let rest = rest.trim();
        let mut it = rest.split_whitespace();
        let num: i64 = it
            .next()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| err(input))?;
        let unit = it.next().unwrap_or("").trim_end_matches('s');
        let delta = match unit {
            "second" => Duration::seconds(num),
            "minute" => Duration::minutes(num),
            "hour" => Duration::hours(num),
            "day" => Duration::days(num),
            _ => return Err(err(input)),
        };
        return Ok((Recurrence::Once, Utc::now() + delta));
    }

    if s == "hourly" || s == "every hour" {
        return Ok((
            Recurrence::Interval { minutes: 60 },
            Utc::now() + Duration::hours(1),
        ));
    }
    if let Some(rest) = s.strip_prefix("every ") {
        let rest = rest.trim();
        if let Some((n, unit)) = parse_interval_phrase(rest) {
            return Ok((
                Recurrence::Interval { minutes: n * unit },
                Utc::now() + Duration::minutes(n * unit),
            ));
        }
    }

    // --- split off a time expression ---
    let (rec_kind, time_str) =
        if let Some(rest) = strip_any(s, &["every day ", "everyday ", "daily "]) {
            (Kind::Daily, strip_at(rest))
        } else if let Some(rest) = strip_any(s, &["weekdays "]) {
            (Kind::Weekdays, strip_at(rest))
        } else if let Some((idx, remainder)) = extract_weekday_prefix(s) {
            (Kind::Weekly(idx), strip_at(remainder))
        } else if let Some(rest) = strip_any(s, &["tomorrow "]) {
            (Kind::Tomorrow, strip_at(rest))
        } else if let Some(rest) = strip_any(s, &["today ", "tonight "]) {
            (Kind::Today, strip_at(rest))
        } else {
            (Kind::Once, strip_at(s))
        };

    let time = parse_time(time_str.trim()).ok_or_else(|| err(input))?;
    let now_date = now_local.date_naive();

    let target = match rec_kind {
        Kind::Today | Kind::Once => {
            let cand = now_date.and_time(time);
            let cand = Local
                .from_local_datetime(&cand)
                .single()
                .ok_or_else(|| err(input))?;
            if cand > now_local {
                cand.with_timezone(&Utc)
            } else {
                // passed today -> tomorrow (for bare times)
                let tmr = now_date
                    .succ_opt()
                    .ok_or_else(|| err(input))?
                    .and_time(time);
                Local
                    .from_local_datetime(&tmr)
                    .single()
                    .ok_or_else(|| err(input))?
                    .with_timezone(&Utc)
            }
        }
        Kind::Tomorrow => {
            let tmr = now_date
                .succ_opt()
                .ok_or_else(|| err(input))?
                .and_time(time);
            Local
                .from_local_datetime(&tmr)
                .single()
                .ok_or_else(|| err(input))?
                .with_timezone(&Utc)
        }
        Kind::Daily => {
            let mut date = now_date;
            loop {
                let cand_local = date.and_time(time);
                if let Some(cand) = Local.from_local_datetime(&cand_local).single() {
                    if cand > now_local {
                        return Ok((Recurrence::Daily { time }, cand.with_timezone(&Utc)));
                    }
                }
                date = date.succ_opt().ok_or_else(|| err(input))?;
            }
        }
        Kind::Weekdays => {
            let mut date = now_date;
            loop {
                let cand_local = date.and_time(time);
                if let Some(cand) = Local.from_local_datetime(&cand_local).single() {
                    if cand > now_local && is_weekday(date.weekday().num_days_from_monday()) {
                        return Ok((Recurrence::Weekdays { time }, cand.with_timezone(&Utc)));
                    }
                }
                date = date.succ_opt().ok_or_else(|| err(input))?;
            }
        }
        Kind::Weekly(day_idx) => {
            let want = day_number(day_name(day_idx)).ok_or_else(|| err(input))?; // 1..=7 Mon..Sun
            let mut date = now_date;
            loop {
                let cand_local = date.and_time(time);
                if let Some(cand) = Local.from_local_datetime(&cand_local).single() {
                    if cand > now_local && date.weekday().number_from_monday() == want {
                        return Ok((
                            Recurrence::Weekly {
                                weekday: want,
                                time,
                            },
                            cand.with_timezone(&Utc),
                        ));
                    }
                }
                date = date.succ_opt().ok_or_else(|| err(input))?;
            }
        }
    };
    // Once/Today/Tomorrow handled inline above
    let recurrence = match rec_kind {
        Kind::Today | Kind::Once | Kind::Tomorrow => Recurrence::Once,
        _ => unreachable!(),
    };
    let _ = recurrence;
    Ok(match rec_kind {
        Kind::Daily => (Recurrence::Daily { time }, target),
        Kind::Weekdays => (Recurrence::Weekdays { time }, target),
        Kind::Weekly(idx) => (
            Recurrence::Weekly {
                weekday: day_number(day_name(idx)).unwrap(),
                time,
            },
            target,
        ),
        _ => (Recurrence::Once, target),
    })
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Once,
    Today,
    Tomorrow,
    Daily,
    Weekdays,
    /// index into DAY_NAMES
    Weekly(usize),
}

const DAY_NAMES: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

fn day_name(idx: usize) -> &'static str {
    DAY_NAMES[idx.min(6)]
}

fn day_number(name: &str) -> Option<u32> {
    DAY_NAMES
        .iter()
        .position(|d| d.starts_with(name))
        .map(|p| p as u32 + 1)
}

/// If s starts with "every <weekday>", return (day index, remainder after the day word).
fn extract_weekday_prefix(s: &str) -> Option<(usize, &str)> {
    if !s.starts_with("every ") {
        return None;
    }
    let rest = s["every ".len()..].trim_start();
    for (idx, d) in DAY_NAMES.iter().enumerate() {
        let full: String = d.to_string();
        let abbr: String = d.chars().take(3).collect();
        let matched = if rest == full || rest == abbr {
            rest.len()
        } else if rest.starts_with(&full) {
            full.len()
        } else if rest.starts_with(&abbr)
            && !rest[3..].starts_with(|c: char| c.is_ascii_alphabetic())
        {
            3
        } else {
            continue;
        };
        return Some((idx, rest[matched..].trim_start()));
    }
    None
}

fn strip_any<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| s.strip_prefix(p))
}

fn strip_at(s: &str) -> &str {
    s.trim().strip_prefix("at ").unwrap_or(s.trim())
}

/// "12am", "9:30pm", "07:15", "noon", "midnight", "10 pm"
fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    match s {
        "noon" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    // hh[:mm] [am|pm]
    let (num_part, pm) = if let Some(rest) = s.strip_suffix("am") {
        (rest.trim(), false)
    } else if let Some(rest) = s.strip_suffix("pm") {
        (rest.trim(), true)
    } else {
        (s, false)
    };
    let mut parts = num_part.split(':');
    let h: u32 = parts.next()?.trim().parse().ok()?;
    let m: u32 = match parts.next() {
        Some(x) => x.trim().parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || m > 59 {
        return None;
    }
    let hour = if pm {
        if h == 12 {
            12
        } else {
            h + 12
        }
    } else if h == 12 {
        0
    } else {
        h
    };
    NaiveTime::from_hms_opt(hour, m, 0)
}

/// "30 min", "2 hours", "90m", "1h" -> (count, minutes_per_unit)
fn parse_interval_phrase(rest: &str) -> Option<(i64, i64)> {
    let rest = rest.trim();
    let mut it = rest.split_whitespace();
    let num: i64 = it.next()?.parse().ok()?;
    let unit = it.next()?.trim_end_matches('s');
    let per = match unit {
        "min" | "minute" | "m" => 1,
        "hour" | "hr" | "h" => 60,
        "day" | "d" => 1440,
        _ => return None,
    };
    Some((num.max(1), per))
}

fn err(input: &str) -> Error {
    Error::Config(format!(
        "could not understand schedule time '{input}'. Try: 'at 12am', 'every day at 6pm', \
         'tomorrow at 9:30am', 'in 2 hours', 'every monday at 8am'"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_times() {
        assert_eq!(parse_time("12am"), NaiveTime::from_hms_opt(0, 0, 0));
        assert_eq!(parse_time("12pm"), NaiveTime::from_hms_opt(12, 0, 0));
        assert_eq!(parse_time("9:30pm"), NaiveTime::from_hms_opt(21, 30, 0));
        assert_eq!(parse_time("07:15"), NaiveTime::from_hms_opt(7, 15, 0));
        assert_eq!(parse_time("noon"), NaiveTime::from_hms_opt(12, 0, 0));
        assert_eq!(parse_time("midnight"), NaiveTime::from_hms_opt(0, 0, 0));
        assert_eq!(parse_time("bogus"), None);
    }

    #[test]
    fn parses_relative_intervals() {
        let (rec, at) = parse_when("in 30 minutes").unwrap();
        assert_eq!(rec, Recurrence::Once);
        assert!(at > Utc::now() + Duration::minutes(29));
        let (_, at) = parse_when("in 2 hours").unwrap();
        assert!(at > Utc::now() + Duration::hours(1));
    }

    #[test]
    fn parses_daily_recurring() {
        let (rec, _) = parse_when("every day at 12am").unwrap();
        assert_eq!(
            rec,
            Recurrence::Daily {
                time: NaiveTime::from_hms_opt(0, 0, 0).unwrap()
            }
        );
        let (rec, _) = parse_when("daily at 6:30pm").unwrap();
        assert_eq!(
            rec,
            Recurrence::Daily {
                time: NaiveTime::from_hms_opt(18, 30, 0).unwrap()
            }
        );
    }

    #[test]
    fn parses_weekly() {
        let (rec, _) = parse_when("every monday at 9am").unwrap();
        assert_eq!(
            rec,
            Recurrence::Weekly {
                weekday: 1,
                time: NaiveTime::from_hms_opt(9, 0, 0).unwrap()
            }
        );
        let (rec, _) = parse_when("every sun 22:00").unwrap();
        assert_eq!(
            rec,
            Recurrence::Weekly {
                weekday: 7,
                time: NaiveTime::from_hms_opt(22, 0, 0).unwrap()
            }
        );
    }

    #[test]
    fn parses_once_today_or_tomorrow() {
        let (rec, at) = parse_when("at 12am").unwrap();
        assert_eq!(rec, Recurrence::Once);
        // must be in the future
        assert!(at > Utc::now());
        let (rec, _) = parse_when("23:59").unwrap();
        assert_eq!(rec, Recurrence::Once);
    }

    #[test]
    fn daily_next_is_in_future() {
        let (rec, first) = parse_when("every day at noon").unwrap();
        assert!(first > Utc::now());
        let second = rec.next_after(first + Duration::seconds(1)).unwrap();
        assert!(second > first);
        // ~24h apart
        let diff = second - first;
        assert!(diff >= Duration::hours(23) && diff <= Duration::hours(25));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_when("whenever feels right").is_err());
        assert!(parse_when("at banana").is_err());
    }
}
