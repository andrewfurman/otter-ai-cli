use chrono::{Datelike, TimeZone};
use serde_json::{json, Value};

use crate::auth::authenticated_client;
use crate::pagination::collect_pages;
use crate::util::{
    api, fail, format_duration, format_timestamp, print_json, result_repr,
    truthy, value_str,
};

const DEFAULT_LIMIT: u32 = 500;
const WIDEN_DAYS_FOR_TITLE_TIME: i64 = 7; // widen server window to catch imported recordings

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Relevant,
    Recent,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    query: Option<String>,
    speakers: Vec<String>,
    from: Option<String>,
    to: Option<String>,
    days: Option<u32>,
    sort: SortMode,
    limit: Option<u32>,
    as_json: bool,
) {
    // Validate arguments before logging in.
    if days.is_some() && (from.is_some() || to.is_some()) {
        fail("--days cannot be combined with --from/--to");
    }
    let (begin, end) = match (from, to, days) {
        (Some(from), to, None) => {
            let begin = start_of_day_et(&from)
                .unwrap_or_else(|| invalid_date(&from, "--from must be YYYY-MM-DD"));
            let end = to
                .as_deref()
                .map(start_of_day_exclusive_et)
                .transpose()
                .unwrap_or_else(|e| e);
            let end = end.unwrap_or_else(|| start_of_day_exclusive_et(&today_yyyy_mm_dd()).unwrap());
            if end <= begin {
                fail("--to must not be earlier than --from");
            }
            (Some(begin), Some(end))
        }
        (None, None, Some(days)) => last_n_calendar_days_window(days),
        (None, None, None) => (None, None),
        (None, Some(_), None) => fail("--to requires --from"),
        (Some(_), None, None) => unreachable!(), // handled above
        (Some(_), _, Some(_)) | (None, Some(_), Some(_)) | (Some(_), Some(_), Some(_)) => {
            unreachable!()
        }
    };
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let relevance = matches!(sort, SortMode::Relevant);
    let speakers: Vec<String> = speakers
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let client = authenticated_client();
    let session_id = format!(
        "cli-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    // When filtering by a calendar date window, widen the server window to catch
    // imported recordings whose upload time differs from title-embedded time.
    let (server_begin, server_end) = widen_server_window(begin, end);

    // Case 1: Date-only window (no keyword, no speaker).
    // Try advanced_search with only begin/end. If it fails, fall back to list.
    if query.as_deref().map_or(true, str::is_empty) && speakers.is_empty() {
        if let (Some(begin), Some(end)) = (server_begin, server_end) {
            let result = api(client.advanced_search(
                None, None, Some(begin), Some(end), limit, relevance, &session_id,
            ));
            if result.ok() && truthy(&result.data["hits"]) {
                let mut hits = result.data["hits"].as_array().cloned().unwrap_or_default();
                if let (Some(user_begin), Some(user_end)) = (Some(begin), Some(end)) {
                    hits = hits
                        .into_iter()
                        .filter(|hit| within_user_window(hit, user_begin, user_end))
                        .collect();
                }
                return output_hits(hits, as_json);
            }
        }
        // Fallback: list speeches window and map to "hits"-like entries.
        return date_only_fallback_list(&client, begin, end, as_json, limit);
    }

    // Case 2: q and/or speaker filters.
    let mut hits: Option<Vec<Value>> = None;
    if speakers.is_empty() {
        // Single search
        let result = api(client.advanced_search(
            query.as_deref(),
            None,
            server_begin,
            server_end,
            limit,
            relevance,
            &session_id,
        ));
        if !result.ok() {
            fail(format!("Search failed: {}", result_repr(&result)));
        }
        hits = Some(result.data["hits"].as_array().cloned().unwrap_or_default());
    } else {
        // Multiple speakers: intersect conversations containing ALL given speakers.
        for speaker in speakers.iter() {
            let result = api(client.advanced_search(
                query.as_deref(),
                Some(speaker),
                server_begin,
                server_end,
                limit,
                relevance,
                &session_id,
            ));
            if !result.ok() {
                fail(format!("Search failed: {}", result_repr(&result)));
            }
            let mut speaker_hits = result.data["hits"].as_array().cloned().unwrap_or_default();
            // Apply user window filtering (uploads vs embedded title time).
            if let (Some(user_begin), Some(user_end)) = (begin, end) {
                speaker_hits = speaker_hits
                    .into_iter()
                    .filter(|hit| within_user_window(hit, user_begin, user_end))
                    .collect();
            }
            hits = Some(match (hits.take(), Some(speaker_hits)) {
                (None, Some(h)) => h,
                (Some(existing), Some(new)) => intersect_by_otid(existing, new),
                (Some(existing), None) => existing,
                (None, None) => Vec::new(),
            });
            if hits.as_ref().is_some_and(|h| h.is_empty()) {
                break;
            }
        }
    }

    let mut hits = hits.unwrap_or_default();
    // Sort locally when requested, since intersections may scramble order.
    if !relevance {
        hits.sort_by_key(|h| {
            h.get("start_time")
                .and_then(Value::as_i64)
                .unwrap_or_default()
        });
        hits.reverse(); // most recent first
    } else {
        hits.sort_by(|a, b| {
            let sa = a.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            let sb = b.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    if hits.len() > limit as usize {
        hits.truncate(limit as usize);
    }
    output_hits(hits, as_json);
}

fn output_hits(mut hits: Vec<Value>, as_json: bool) {
    if as_json {
        print_json(&json!({ "hits": hits }));
        return;
    }
    if hits.is_empty() {
        println!("No conversations found.");
        return;
    }
    println!("Found {} conversations:\n", hits.len());
    for hit in &mut hits {
        let title = value_str(&hit["title"]);
        let otid = value_str(&hit["speech_otid"]);
        let date = format_timestamp(&hit["start_time"]);
        let duration = format_duration(&hit["duration"]);
        let snippet = match_snippet(hit);
        let mut line = format!("{date}  {duration:>6}  {title}  ({otid})");
        if let Some(snippet) = snippet {
            line.push_str(&format!("\n   {snippet}"));
        }
        println!("{line}\n");
    }
}

fn date_only_fallback_list(
    client: &otter::Client,
    begin: Option<i64>,
    end: Option<i64>,
    as_json: bool,
    limit: u32,
) {
    // Collect listing pages starting from server_begin (cutoff) and then apply end filter.
    let cutoff = begin.map(|b| b as f64);
    let listing = collect_pages(true, cutoff, 1000, |cursor| {
        client.get_speeches_page("0", 100, "owned", cursor)
    });
    let mut speeches = listing.data["speeches"].as_array().cloned().unwrap_or_default();
    if let Some(end) = end {
        speeches.retain(|s| {
            s.get("created_at")
                .and_then(Value::as_i64)
                .is_some_and(|t| t < end)
        });
    }
    if speeches.len() > limit as usize {
        speeches.truncate(limit as usize);
    }
    // Map to hits-like shape.
    let hits: Vec<Value> = speeches
        .into_iter()
        .map(|s| {
            json!({
                "speech_otid": s["otid"],
                "title": s["title"],
                "duration": s["duration"],
                "start_time": s["created_at"],
                "matched_transcripts": [],
            })
        })
        .collect();
    output_hits(hits, as_json);
}

fn start_of_day_et(date: &str) -> Option<i64> {
    let (year, month, day) = split_ymd(date)?;
    match chrono_tz::America::New_York.with_ymd_and_hms(year, month, day, 0, 0, 0) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            Some(dt.timestamp())
        }
        _ => None,
    }
}

fn start_of_day_exclusive_et(date: &str) -> Result<i64, !> {
    let (year, month, day) = split_ymd(date).unwrap_or_else(|| invalid_date(date, "invalid date"));
    let next = chrono::NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .succ_opt()
        .unwrap();
    match chrono_tz::America::New_York
        .with_ymd_and_hms(next.year(), next.month(), next.day(), 0, 0, 0)
    {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            Ok(dt.timestamp())
        }
        _ => Ok(0),
    }
}

fn split_ymd(date: &str) -> Option<(i32, u32, u32)> {
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (y, m, d) = (y.parse::<i32>().ok()?, m.parse::<u32>().ok()?, d.parse::<u32>().ok()?);
    Some((y, m, d))
}

fn invalid_date(date: &str, message: &str) -> ! {
    fail(format!("{message}: {date}"));
}

fn today_yyyy_mm_dd() -> String {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now = chrono_tz::America::New_York
        .timestamp_opt(now_secs, 0)
        .single()
        .unwrap();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

fn last_n_calendar_days_window(days: u32) -> (Option<i64>, Option<i64>) {
    // Inclusive start at midnight ET N-1 days ago; exclusive end at midnight ET tomorrow.
    let tz = chrono_tz::America::New_York;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now = tz.timestamp_opt(now_secs, 0).single().unwrap().date_naive();
    let start = now
        .pred_opt()
        .map(|_| ())
        .and(Some(now - chrono::Duration::days(i64::from(days.saturating_sub(1)))))
        .unwrap_or(now);
    let start_ts = match tz.with_ymd_and_hms(start.year(), start.month(), start.day(), 0, 0, 0) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        _ => 0,
    };
    let tomorrow = now.succ_opt().unwrap_or(now);
    let end_ts = match tz.with_ymd_and_hms(tomorrow.year(), tomorrow.month(), tomorrow.day(), 0, 0, 0)
    {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        _ => 0,
    };
    (Some(start_ts), Some(end_ts))
}

fn widen_server_window(begin: Option<i64>, end: Option<i64>) -> (Option<i64>, Option<i64>) {
    let begin = begin.map(|b| b.saturating_sub(WIDEN_DAYS_FOR_TITLE_TIME * 86400));
    (begin, end)
}

fn within_user_window(hit: &Value, user_begin: i64, user_end: i64) -> bool {
    let start = hit.get("start_time").and_then(Value::as_i64).unwrap_or(0);
    if (user_begin..user_end).contains(&start) {
        return true;
    }
    if let Some(parsed) = parse_title_time_et(&value_str(&hit["title"])) {
        return (user_begin..user_end).contains(&parsed);
    }
    false
}

fn parse_title_time_et(title: &str) -> Option<i64> {
    // Suffix: " on Mon Sep 21st 2026 @ 7:37am ET"
    let idx = title.rfind(" on ")?;
    let suffix = &title[idx + 4..];
    if !suffix.ends_with(" ET") {
        return None;
    }
    let suffix = &suffix[..suffix.len() - 3]; // drop " ET"
    let (left, time) = suffix.rsplit_once(" @ ")?;
    // Left: "Mon Sep 21st 2026"
    let mut parts = left.split_whitespace();
    let _dow = parts.next()?; // "Mon"
    let mon = parts.next()?;
    let day_ordinal = parts.next()?;
    let year = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let month = match_mon(mon)?;
    let day: u32 = day_ordinal.trim_end_matches(['s', 't', 'n', 'd', 'r', 'h']).parse().ok()?;
    let year: i32 = year.parse().ok()?;
    // time: "7:37am" or "10:05pm"
    let (hm, ampm) = time.split_at(time.len().saturating_sub(2));
    let (h, m) = hm.split_once(':')?;
    let mut hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    let ampm = ampm.to_ascii_lowercase();
    match ampm.as_str() {
        "am" => {
            if hour == 12 {
                hour = 0;
            }
        }
        "pm" => {
            if hour != 12 {
                hour += 12;
            }
        }
        _ => return None,
    }
    match chrono_tz::America::New_York.with_ymd_and_hms(year, month, day, hour, minute, 0) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            Some(dt.timestamp())
        }
        _ => None,
    }
}

fn match_mon(mon: &str) -> Option<u32> {
    match mon {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn match_snippet(hit: &Value) -> Option<String> {
    if hit.get("matched_title").and_then(Value::as_bool).unwrap_or(false) {
        let title = value_str(&hit["title"]);
        if !title.is_empty() {
            return Some(ellipsize(&title, 160));
        }
    }
    if let Some(items) = hit.get("matched_transcripts").and_then(Value::as_array) {
        for item in items {
            let text = value_str(&item["matched_transcript"]);
            if !text.is_empty() {
                return Some(ellipsize(&text, 160));
            }
        }
    }
    None
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut result = String::new();
    for c in text.chars().take(max - 1) {
        result.push(c);
    }
    result.push('…');
    result
}

fn intersect_by_otid(a: Vec<Value>, b: Vec<Value>) -> Vec<Value> {
    use std::collections::HashSet;
    let set_b: HashSet<String> = b
        .iter()
        .map(|h| value_str(&h["speech_otid"]))
        .filter(|s| !s.is_empty())
        .collect();
    a.into_iter()
        .filter(|h| set_b.contains(&value_str(&h["speech_otid"])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    #[test]
    fn title_time_parser_extracts_expected_epoch() {
        // "Mon Sep 21st 2026 @ 7:37am ET" -> check hour/min conversion and ordinal trim
        let title = "Call with A on Mon Sep 21st 2026 @ 7:37am ET";
        let ts = parse_title_time_et(title).expect("parse title time");
        let dt = chrono_tz::America::New_York.timestamp_opt(ts, 0).single().unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 9);
        assert_eq!(dt.day(), 21);
        assert_eq!((dt.hour(), dt.minute()), (7, 37));
        // PM
        let title = "Update on Tue Oct 3rd 2026 @ 12:05pm ET";
        let ts = parse_title_time_et(title).expect("parse title time");
        let dt = chrono_tz::America::New_York.timestamp_opt(ts, 0).single().unwrap();
        assert_eq!((dt.hour(), dt.minute()), (12, 5));
        let title = "Evening on Tue Oct 3rd 2026 @ 7:05pm ET";
        let ts = parse_title_time_et(title).expect("parse title time");
        let dt = chrono_tz::America::New_York.timestamp_opt(ts, 0).single().unwrap();
        assert_eq!((dt.hour(), dt.minute()), (19, 5));
    }

    #[test]
    fn inclusive_end_date_is_next_day_midnight_et_and_dst_boundaries() {
        // 2024-03-10 is DST start date; midnight exclusive end is 2024-03-11 00:00 ET.
        let begin = start_of_day_et("2024-03-10").unwrap();
        let end = start_of_day_exclusive_et("2024-03-10").unwrap();
        assert!(end > begin);
        // 2024-11-03 is DST end date; still compute midnight next day in ET.
        let begin = start_of_day_et("2024-11-03").unwrap();
        let end = start_of_day_exclusive_et("2024-11-03").unwrap();
        assert!(end > begin);
    }
}

