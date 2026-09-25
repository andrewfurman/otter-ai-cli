use chrono::{Datelike, TimeZone};
use serde_json::{json, Value};

use crate::auth::authenticated_client;
use crate::pagination::collect_pages;
use crate::util::{api, fail, format_duration, format_timestamp, print_json, truthy, value_str};
use otter::client::AdvancedSearchOptions as SearchReqOpts;

const DEFAULT_LIMIT: u32 = 500;
const WIDEN_DAYS_FOR_TITLE_TIME: i64 = 14; // extend end to catch imported recordings

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Relevant,
    Recent,
}

pub struct SearchOptions {
    pub query: Option<String>,
    pub speakers: Vec<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub days: Option<u32>,
    pub sort: SortMode,
    pub limit: Option<u32>,
    pub as_json: bool,
    pub debug: bool,
    pub tries: u32,
    pub max_seconds: u32,
}

pub fn run(options: SearchOptions) {
    let SearchOptions {
        query,
        speakers,
        from,
        to,
        days,
        sort,
        limit,
        as_json,
        debug,
        tries,
        max_seconds,
    } = options;
    // Validate arguments before logging in.
    if days.is_some() && (from.is_some() || to.is_some()) {
        fail("--days cannot be combined with --from/--to");
    }
    if to.is_some() && from.is_none() {
        fail("--to requires --from");
    }
    let (begin, end) = if let Some(days) = days {
        last_n_calendar_days_window(days)
    } else if let Some(from) = from {
        let begin = start_of_day_et(&from)
            .unwrap_or_else(|| invalid_date(&from, "--from must be YYYY-MM-DD"));
        let end = to
            .as_deref()
            .map(|t| {
                start_of_day_exclusive_et(t)
                    .unwrap_or_else(|| invalid_date(t, "--to must be YYYY-MM-DD"))
            })
            .unwrap_or_else(|| {
                start_of_day_exclusive_et(&today_yyyy_mm_dd()).expect("today is valid")
            });
        if end <= begin {
            fail("--to must not be earlier than --from");
        }
        (Some(begin), Some(end))
    } else {
        (None, None)
    };
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let relevance = matches!(sort, SortMode::Relevant);
    let speakers: Vec<String> = speakers
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let client = authenticated_client();

    // For searches with query/speakers, do not send an end_date; filter locally by recorded_at.
    // For date-only searches, attempt advanced_search with a begin (and possibly end) first.
    let (server_begin, server_end) =
        if query.as_deref().is_none_or(str::is_empty) && speakers.is_empty() {
            widen_server_window(begin, end)
        } else {
            (begin, None)
        };

    // Case 1: Date-only window (no keyword, no speaker).
    // Try advanced_search with only begin/end. If it fails, fall back to list.
    if query.as_deref().is_none_or(str::is_empty) && speakers.is_empty() {
        if let (Some(begin), Some(end)) = (server_begin, server_end) {
            let result = api(client.advanced_search_opts(SearchReqOpts {
                query: None,
                speaker: None,
                begin_date: Some(begin),
                end_date: Some(end),
                size: limit,
                relevance,
                session_id: &crate::util::uuid_v4(),
            }));
            if result.ok() && truthy(&result.data["hits"]) {
                let mut hits = result.data["hits"].as_array().cloned().unwrap_or_default();
                hits = apply_user_window_filter(hits, begin, end);
                return output_hits(hits, as_json);
            }
        }
        // Fallback: list speeches window and map to "hits"-like entries.
        return date_only_fallback_list(&client, begin, end, as_json, limit);
    }

    // Case 2: q and/or speaker filters.
    let mut hits: Option<Vec<Value>> = None;
    if speakers.is_empty() && query.as_deref().is_some_and(|q| !q.trim().is_empty()) {
        // Keyword-only: union several nondeterministic calls with fresh session ids.
        let mut list = union_advanced_search_tries(
            &client,
            MultiTryOpts {
                query: query.as_deref(),
                speaker: None,
                begin_date: server_begin,
                end_date: server_end,
                size: limit,
                relevance,
                tries,
                debug,
            },
        );
        if let (Some(b), Some(e)) = (begin, end) {
            list = apply_user_window_filter(list, b, e);
        }
        hits = Some(list);
    } else if !speakers.is_empty() && query.as_deref().is_none_or(str::is_empty) {
        // Speaker-only with no dates: windowed advanced_search union, walking back in time.
        let mut total: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        let start_clock = std::time::Instant::now();
        let window_days: i64 = 90;
        let window_secs = window_days * 86400;
        let mut window_end = start_of_day_exclusive_et(&today_yyyy_mm_dd()).unwrap_or(0);
        let floor_ts = window_end.saturating_sub(10 * 365 * 86400); // ~10 years back
        let mut empty_windows = 0u32;
        let stop_after_empty = 3u32;
        while window_end > floor_ts {
            if start_clock.elapsed().as_secs() >= max_seconds as u64 {
                let covered_to = format_timestamp(&json!(window_end));
                eprintln!(
                    "Time budget reached ({}s). Coverage so far back to {}.",
                    max_seconds, covered_to
                );
                break;
            }
            let window_begin = window_end.saturating_sub(window_secs);
            let mut window_hits_opt: Option<Vec<Value>> = None;
            for speaker in speakers.iter() {
                let mut sh = union_advanced_search_tries(
                    &client,
                    MultiTryOpts {
                        query: None,
                        speaker: Some(speaker.as_str()),
                        begin_date: Some(window_begin),
                        end_date: Some(window_end),
                        size: DEFAULT_LIMIT,
                        relevance,
                        tries,
                        debug,
                    },
                );
                sh = apply_user_window_filter(sh, window_begin, window_end);
                window_hits_opt = Some(match (window_hits_opt.take(), Some(sh)) {
                    (None, Some(h)) => h,
                    (Some(existing), Some(new)) => intersect_by_otid(existing, new),
                    (Some(existing), None) => existing,
                    (None, None) => Vec::new(),
                });
                if window_hits_opt.as_ref().is_some_and(|h| h.is_empty()) {
                    break;
                }
            }
            let window_hits = window_hits_opt.unwrap_or_default();
            let mut new_count = 0usize;
            for hit in window_hits {
                let otid = value_str(&hit["speech_otid"]);
                if otid.is_empty() {
                    continue;
                }
                match total.get_mut(&otid) {
                    Some(existing) => {
                        if merge_hit(existing, &hit) {
                            // merged details
                        }
                    }
                    None => {
                        total.insert(otid, hit);
                        new_count += 1;
                    }
                }
            }
            eprintln!(
                "window {}..{}: +{} (total {})",
                format_timestamp(&json!(window_begin)),
                format_timestamp(&json!(window_end)),
                new_count,
                total.len()
            );
            if new_count == 0 {
                empty_windows += 1;
            } else {
                empty_windows = 0;
            }
            if empty_windows >= stop_after_empty {
                break;
            }
            window_end = window_begin;
        }
        hits = Some(total.into_values().collect());
    } else {
        // Keyword + speaker(s): union several calls per speaker, then intersect conversations
        // containing ALL given speakers.
        for speaker in speakers.iter() {
            let server_size = DEFAULT_LIMIT;
            let mut speaker_hits = union_advanced_search_tries(
                &client,
                MultiTryOpts {
                    query: query.as_deref(),
                    speaker: Some(speaker.as_str()),
                    begin_date: server_begin,
                    end_date: server_end,
                    size: server_size,
                    relevance,
                    tries,
                    debug,
                },
            );
            if let (Some(b), Some(e)) = (begin, end) {
                speaker_hits = apply_user_window_filter(speaker_hits, b, e);
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
            // Prefer parsed recording time; fall back to upload time.
            parse_title_time_et(&value_str(&h["title"]))
                .or_else(|| h.get("start_time").and_then(Value::as_i64))
                .unwrap_or_default()
        });
        hits.reverse(); // most recent first
    } else {
        // Deterministic: sort by score desc (accept `score` or `_score`), then recorded_at desc.
        let any_score = hits.iter().any(|h| score_of(h).is_some());
        if any_score {
            hits.sort_by(|a, b| {
                let sa = score_of(a).unwrap_or(f64::MIN);
                let sb = score_of(b).unwrap_or(f64::MIN);
                match sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal) {
                    std::cmp::Ordering::Equal => {
                        let ra = a.get("recorded_at").and_then(Value::as_i64).unwrap_or(0);
                        let rb = b.get("recorded_at").and_then(Value::as_i64).unwrap_or(0);
                        rb.cmp(&ra)
                    }
                    other => other,
                }
            });
        }
    }
    if hits.len() > limit as usize {
        hits.truncate(limit as usize);
    }
    output_hits(hits, as_json);
}

fn add_recorded_at(hit: &mut Value) {
    let start = hit.get("start_time").and_then(Value::as_i64).unwrap_or(0);
    let recorded = parse_title_time_et(&value_str(&hit["title"])).unwrap_or(start);
    hit["recorded_at"] = json!(recorded);
}

fn score_of(hit: &Value) -> Option<f64> {
    hit.get("score")
        .and_then(Value::as_f64)
        .or_else(|| hit.get("_score").and_then(Value::as_f64))
}

fn output_hits(mut hits: Vec<Value>, as_json: bool) {
    for h in &mut hits {
        add_recorded_at(h);
    }
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
        // Prefer parsed recording time for display.
        let date = format_timestamp(&hit["recorded_at"]);
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
    eprintln!(
        "Note: advanced_search with date-only failed; falling back to listing and local filtering."
    );
    // Collect listing pages starting from server_begin (cutoff) and then apply end filter.
    let cutoff = begin.map(|b| b as f64);
    let listing = collect_pages(true, cutoff, 1000, |cursor| {
        client.get_speeches_page("0", 100, "owned", cursor)
    });
    let speeches = listing.data["speeches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    // Map to hits-like shape and apply user-aware window filtering (title time or upload time).
    let mut hits: Vec<Value> = speeches
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
    if let (Some(b), Some(e)) = (begin, end) {
        hits = apply_user_window_filter(hits, b, e);
    }
    if hits.len() > limit as usize {
        hits.truncate(limit as usize);
    }
    output_hits(hits, as_json);
}

struct MultiTryOpts<'a> {
    query: Option<&'a str>,
    speaker: Option<&'a str>,
    begin_date: Option<i64>,
    end_date: Option<i64>,
    size: u32,
    relevance: bool,
    tries: u32,
    debug: bool,
}

fn union_advanced_search_tries(client: &otter::Client, opts: MultiTryOpts<'_>) -> Vec<Value> {
    use std::collections::HashMap;
    use std::thread::sleep;
    use std::time::Duration;
    let tries = opts.tries.clamp(1, 10);
    let mut by_otid: HashMap<String, Value> = HashMap::new();
    for attempt in 0..tries {
        let session = crate::util::uuid_v4();
        let req = SearchReqOpts {
            query: opts.query,
            speaker: opts.speaker,
            begin_date: opts.begin_date,
            end_date: opts.end_date,
            size: opts.size,
            relevance: opts.relevance,
            session_id: &session,
        };
        let result = api(client.advanced_search_opts(req));
        if should_log_progress(opts.debug) {
            // URL is built only for debug display.
            use otter::client::DEFAULT_ADVANCED_SEARCH_PARAMS as NAMES;
            let built = client
                .advanced_search_request_with_params(req, NAMES)
                .build()
                .ok()
                .map(|r| r.url().to_string())
                .unwrap_or_else(|| "<unavailable>".to_string());
            debug_print(&built, &result);
        }
        let hits = result.data["hits"].as_array().cloned().unwrap_or_default();
        let mut new_this_round = 0usize;
        for hit in hits {
            let otid = value_str(&hit["speech_otid"]);
            if otid.is_empty() {
                continue;
            }
            match by_otid.get_mut(&otid) {
                Some(existing) => {
                    if merge_hit(existing, &hit) {
                        // merged additional snippets or better score; not counted as "new"
                    }
                }
                None => {
                    by_otid.insert(otid, hit);
                    new_this_round += 1;
                }
            }
        }
        let round_hits = result.data["hits"].as_array().map_or(0, Vec::len);
        if should_log_progress(opts.debug) {
            eprintln!(
                "search try {}/{}: hits={} new={} total={}",
                attempt + 1,
                tries,
                round_hits,
                new_this_round,
                by_otid.len()
            );
        }
        if attempt + 1 < tries {
            sleep(Duration::from_millis(3500));
        }
    }
    if by_otid.is_empty() {
        // Suspicious: two extra attempts with longer spacing.
        for extra in 0..2 {
            sleep(Duration::from_millis(5000 + extra * 2000));
            let session = crate::util::uuid_v4();
            let req = SearchReqOpts {
                query: opts.query,
                speaker: opts.speaker,
                begin_date: opts.begin_date,
                end_date: opts.end_date,
                size: opts.size,
                relevance: opts.relevance,
                session_id: &session,
            };
            let result = api(client.advanced_search_opts(req));
            if should_log_progress(opts.debug) {
                use otter::client::DEFAULT_ADVANCED_SEARCH_PARAMS as NAMES;
                let built = client
                    .advanced_search_request_with_params(req, NAMES)
                    .build()
                    .ok()
                    .map(|r| r.url().to_string())
                    .unwrap_or_else(|| "<unavailable>".to_string());
                debug_print(&built, &result);
            }
            let hits = result.data["hits"].as_array().cloned().unwrap_or_default();
            for hit in hits {
                let otid = value_str(&hit["speech_otid"]);
                if otid.is_empty() {
                    continue;
                }
                match by_otid.get_mut(&otid) {
                    Some(existing) => {
                        let _ = merge_hit(existing, &hit);
                    }
                    None => {
                        by_otid.insert(otid, hit);
                    }
                }
            }
            if !by_otid.is_empty() {
                break;
            }
        }
    }
    by_otid.into_values().collect()
}

fn merge_hit(existing: &mut Value, new_hit: &Value) -> bool {
    let mut changed = false;
    // score: keep max (supports `score` and `_score`; stored as `score`)
    let es = score_of(existing).unwrap_or(f64::MIN);
    let ns = score_of(new_hit).unwrap_or(f64::MIN);
    if ns > es {
        existing["score"] = json!(ns);
        changed = true;
    }
    // matched_title: OR
    let mt = existing
        .get("matched_title")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || new_hit
            .get("matched_title")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if mt {
        existing["matched_title"] = json!(true);
        changed = true;
    }
    // matched_transcripts: union by matched_transcript string when present
    let mut seen: std::collections::HashSet<String> = existing["matched_transcripts"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|it| {
            it.get("matched_transcript")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .collect();
    let mut merged = existing["matched_transcripts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if let Some(additional) = new_hit["matched_transcripts"].as_array() {
        for it in additional {
            if let Some(text) = it.get("matched_transcript").and_then(Value::as_str) {
                if seen.insert(text.to_string()) {
                    merged.push(it.clone());
                    changed = true;
                }
            } else {
                // Could not key it; append if not already present as identical object
                if !merged.contains(it) {
                    merged.push(it.clone());
                    changed = true;
                }
            }
        }
    }
    if changed {
        existing["matched_transcripts"] = Value::Array(merged);
    }
    changed
}

fn should_log_progress(debug: bool) -> bool {
    if debug {
        return true;
    }
    use std::io::{self, IsTerminal};
    io::stderr().is_terminal()
}

// advanced_search does not provide pagination; multiple non-deterministic calls are unioned instead.

fn debug_print(url: &str, result: &otter::ApiResponse) {
    let masked = mask_session(url);
    let mut lines = Vec::new();
    lines.push(format!("GET {masked} -> {}", result.status));
    match &result.data {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                let v = &map[&k];
                match v {
                    Value::Array(a) => lines.push(format!("{k}: [array, len={}]", a.len())),
                    Value::Object(o) => lines.push(format!("{k}: {{object, keys={}}}", o.len())),
                    Value::String(s) => {
                        if k != "hits" {
                            lines.push(format!("{k}: \"{}\"", s));
                        }
                    }
                    other => {
                        if k != "hits" {
                            lines.push(format!("{k}: {}", other));
                        }
                    }
                }
            }
        }
        other => {
            lines.push(format!("data: {other}"));
        }
    }
    eprintln!("{}", lines.join("\n"));
}

fn mask_session(url: &str) -> String {
    let mut out = String::new();
    if let Ok(mut parsed) = url::Url::parse(url) {
        let mut pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (k, v) in &mut pairs {
            if k == "session_id" {
                *v = "****".into();
            }
        }
        parsed
            .query_pairs_mut()
            .clear()
            .extend_pairs(pairs.iter().map(|(k, v)| (&k[..], &v[..])));
        out = parsed.to_string();
    }
    if out.is_empty() {
        url.to_string()
    } else {
        out
    }
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

fn start_of_day_exclusive_et(date: &str) -> Option<i64> {
    let (year, month, day) = split_ymd(date)?;
    let next = chrono::NaiveDate::from_ymd_opt(year, month, day)?.succ_opt()?;
    match chrono_tz::America::New_York.with_ymd_and_hms(
        next.year(),
        next.month(),
        next.day(),
        0,
        0,
        0,
    ) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            Some(dt.timestamp())
        }
        _ => None,
    }
}

fn split_ymd(date: &str) -> Option<(i32, u32, u32)> {
    let mut parts = date.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (y, m, d) = (
        y.parse::<i32>().ok()?,
        m.parse::<u32>().ok()?,
        d.parse::<u32>().ok()?,
    );
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
        .and(Some(
            now - chrono::Duration::days(i64::from(days.saturating_sub(1))),
        ))
        .unwrap_or(now);
    let start_ts = match tz.with_ymd_and_hms(start.year(), start.month(), start.day(), 0, 0, 0) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        _ => 0,
    };
    let tomorrow = now.succ_opt().unwrap_or(now);
    let end_ts =
        match tz.with_ymd_and_hms(tomorrow.year(), tomorrow.month(), tomorrow.day(), 0, 0, 0) {
            chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
                dt.timestamp()
            }
            _ => 0,
        };
    (Some(start_ts), Some(end_ts))
}

fn widen_server_window(begin: Option<i64>, end: Option<i64>) -> (Option<i64>, Option<i64>) {
    let end = end.map(|e| e.saturating_add(WIDEN_DAYS_FOR_TITLE_TIME * 86400));
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

fn apply_user_window_filter(hits: Vec<Value>, user_begin: i64, user_end: i64) -> Vec<Value> {
    hits.into_iter()
        .filter(|hit| within_user_window(hit, user_begin, user_end))
        .collect()
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
    let day: u32 = day_ordinal
        .trim_end_matches(['s', 't', 'n', 'd', 'r', 'h'])
        .parse()
        .ok()?;
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
    if hit
        .get("matched_title")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
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
        let dt = chrono_tz::America::New_York
            .timestamp_opt(ts, 0)
            .single()
            .unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 9);
        assert_eq!(dt.day(), 21);
        assert_eq!((dt.hour(), dt.minute()), (7, 37));
        // PM
        let title = "Update on Tue Oct 3rd 2026 @ 12:05pm ET";
        let ts = parse_title_time_et(title).expect("parse title time");
        let dt = chrono_tz::America::New_York
            .timestamp_opt(ts, 0)
            .single()
            .unwrap();
        assert_eq!((dt.hour(), dt.minute()), (12, 5));
        let title = "Evening on Tue Oct 3rd 2026 @ 7:05pm ET";
        let ts = parse_title_time_et(title).expect("parse title time");
        let dt = chrono_tz::America::New_York
            .timestamp_opt(ts, 0)
            .single()
            .unwrap();
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

    #[test]
    fn widen_server_extends_end_only() {
        let begin = Some(1_700_000_000);
        let end_val = 1_700_086_400; // +1 day
        let (wb, we) = widen_server_window(begin, Some(end_val));
        assert_eq!(wb, begin);
        assert!(we.unwrap() > end_val);
    }

    #[test]
    fn recorded_time_used_for_display_and_json() {
        let mut hit = json!({
            "title": "Recording on Mon Sep 21st 2026 @ 7:37am ET",
            "speech_otid": "otid",
            "start_time": 0,
            "duration": 60
        });
        add_recorded_at(&mut hit);
        let ts = hit["recorded_at"].as_i64().unwrap();
        let dt = chrono_tz::America::New_York
            .timestamp_opt(ts, 0)
            .single()
            .unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2026, 9, 21));
    }

    #[test]
    fn window_filter_honors_title_time_when_upload_outside() {
        let begin = start_of_day_et("2026-09-21").unwrap();
        let end = start_of_day_exclusive_et("2026-09-24").unwrap();
        let inside = json!({
            "title": "Talk on Tue Sep 22nd 2026 @ 10:00am ET",
            "speech_otid": "A",
            "start_time": begin - 10_000, // upload earlier
        });
        let outside = json!({
            "title": "Talk",
            "speech_otid": "B",
            "start_time": end + 10_000, // upload later, no title time
        });
        let filtered = apply_user_window_filter(vec![inside, outside], begin, end);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["speech_otid"], "A");
    }
}
