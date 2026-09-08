use std::collections::HashSet;

use otter::{ApiResponse, Error};
use serde_json::{json, Value};

use crate::util::result_repr;

pub struct Listing {
    pub data: Value,
    pub complete: bool,
    pub error: Option<String>,
}

/// Fetch through a single authenticated client's closure. Never retry requests;
/// return partial results and an explicit error if the requested scope is incomplete.
pub fn collect_pages(
    paginate: bool,
    cutoff: Option<f64>,
    max_pages: u32,
    mut fetch: impl FnMut(Option<u64>) -> Result<ApiResponse, Error>,
) -> Listing {
    let mut data = json!({"status": "OK", "end_of_list": false});
    let mut speeches = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut pages = 0;
    let mut complete = false;
    let mut error = None;
    let mut retry_after = None;
    let reason;
    loop {
        if pages >= max_pages {
            reason = "page_limit";
            error = Some(format!(
                "Stopped at --max-pages {max_pages}; results are incomplete."
            ));
            break;
        }
        let result = match fetch(cursor) {
            Ok(result) if result.ok() => result,
            Ok(result) => {
                retry_after = result.retry_after_seconds;
                error = Some(format!(
                    "Listing stopped; results are incomplete: {}",
                    result_repr(&result)
                ));
                reason = "api_error";
                break;
            }
            Err(err) => {
                error = Some(format!("Listing stopped; results are incomplete: {err}"));
                reason = "transport_error";
                break;
            }
        };
        let Some(page) = result.data["speeches"].as_array() else {
            reason = "invalid_page";
            error = Some("Listing response has no speeches array; results are incomplete.".into());
            break;
        };
        if page.iter().any(|speech| {
            speech["otid"].as_str().is_none_or(str::is_empty)
                || (cutoff.is_some()
                    && speech["created_at"]
                        .as_f64()
                        .is_none_or(|time| !time.is_finite() || time <= 0.0))
        }) {
            reason = "invalid_page";
            error = Some(
                "Listing contains an invalid OTID or creation timestamp; results are incomplete."
                    .into(),
            );
            break;
        }
        for speech in page {
            if seen.insert(speech["otid"].as_str().unwrap().to_owned()) {
                speeches.push(speech.clone());
            }
        }
        pages += 1;
        data = result.data;
        match data["end_of_list"].as_bool() {
            Some(true) => {
                complete = true;
                reason = "end_of_list";
                break;
            }
            _ if !paginate => {
                reason = "single_page";
                break;
            }
            None => {
                reason = "invalid_page";
                error =
                    Some("Otter omitted a valid end_of_list flag; results are incomplete.".into());
                break;
            }
            Some(false) => {}
        }
        let Some(next) = data["last_load_ts"].as_u64().filter(|next| *next > 0) else {
            reason = "invalid_cursor";
            error = Some("Otter omitted a valid pagination cursor; results are incomplete.".into());
            break;
        };
        if cursor.is_some_and(|previous| next >= previous) {
            reason = "stalled_cursor";
            error = Some("Otter's cursor did not move backwards; results are incomplete.".into());
            break;
        }
        // This endpoint's cursor is the creation-time upper bound of its next
        // historical page. --days retains its existing created_at semantics.
        if cutoff.is_some_and(|cutoff| (next as f64) < cutoff) {
            complete = true;
            reason = "date_cutoff";
            break;
        }
        cursor = Some(next);
    }
    let fetched = speeches.len();
    if let Some(cutoff) = cutoff {
        speeches.retain(|speech| speech["created_at"].as_f64().unwrap() >= cutoff);
    }
    if error.is_some() {
        data["status"] = json!("failed");
    }
    data["speeches"] = json!(speeches);
    data["pagination"] = json!({
        "complete": complete,
        "scope": if cutoff.is_some() { "date_window" } else { "all" },
        "created_after": cutoff,
        "pages_fetched": pages,
        "unique_fetched": fetched,
        "stop_reason": reason,
        "error": error,
        "retry_after_seconds": retry_after,
    });
    Listing {
        data,
        complete,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(ids: &[(&str, u64)], cursor: u64, end: bool) -> ApiResponse {
        ApiResponse {
            status: 200,
            retry_after_seconds: None,
            data: json!({"status": "OK", "speeches": ids.iter().map(|(id,time)| json!({"otid": id, "created_at": time})).collect::<Vec<_>>(), "last_load_ts": cursor, "end_of_list": end}),
        }
    }

    #[test]
    fn date_window_follows_cursors_deduplicates_and_includes_boundary() {
        let mut calls = Vec::new();
        let listing = collect_pages(true, Some(200.0), 10, |cursor| {
            calls.push(cursor);
            Ok(match cursor {
                None => page(&[("a", 400), ("b", 300)], 299, false),
                Some(299) => page(&[("a", 400), ("c", 200), ("old", 190)], 189, false),
                _ => panic!("must stop at the date boundary"),
            })
        });
        assert_eq!(calls, [None, Some(299)]);
        assert!(listing.complete && listing.error.is_none());
        assert_eq!(listing.data["speeches"].as_array().unwrap().len(), 3);
        assert_eq!(listing.data["speeches"][2]["otid"], "c");
        assert_eq!(listing.data["pagination"]["stop_reason"], "date_cutoff");
        assert_eq!(listing.data["end_of_list"], false);
    }

    #[test]
    fn full_archive_continues_through_overlap_and_empty_final_page() {
        let mut pages = [
            page(&[("a", 400)], 399, false),
            page(&[("a", 400), ("b", 200)], 199, false),
            page(&[], 100, true),
        ]
        .into_iter();
        let listing = collect_pages(true, None, 10, |_| Ok(pages.next().unwrap()));
        assert!(listing.complete);
        assert_eq!(listing.data["speeches"].as_array().unwrap().len(), 2);
        assert_eq!(listing.data["pagination"]["pages_fetched"], 3);
    }

    #[test]
    fn single_page_stays_explicitly_incomplete() {
        let listing = collect_pages(false, None, 10, |_| Ok(page(&[("a", 400)], 399, false)));
        assert!(!listing.complete);
        assert!(listing.error.is_none());
        assert_eq!(listing.data["pagination"]["stop_reason"], "single_page");
    }

    #[test]
    fn loops_missing_cursors_and_page_limits_preserve_partial_results() {
        for (second, max_pages, expected) in [
            (page(&[("b", 300)], 399, false), 10, "stalled_cursor"),
            (page(&[("b", 300)], 500, false), 10, "stalled_cursor"),
            (page(&[("b", 300)], 0, false), 10, "invalid_cursor"),
            (page(&[("b", 300)], 299, false), 1, "page_limit"),
        ] {
            let mut pages = [page(&[("a", 400)], 399, false), second].into_iter();
            let listing = collect_pages(true, None, max_pages, |_| Ok(pages.next().unwrap()));
            assert!(!listing.complete && listing.error.is_some());
            assert_eq!(listing.data["status"], "failed");
            assert_eq!(listing.data["speeches"][0]["otid"], "a");
            assert_eq!(listing.data["pagination"]["stop_reason"], expected);
        }
    }

    #[test]
    fn rate_limits_and_transport_errors_stop_without_retrying() {
        for transport in [false, true] {
            let mut calls = 0;
            let listing = collect_pages(true, None, 10, |_| {
                calls += 1;
                if calls == 1 {
                    return Ok(page(&[("a", 400)], 399, false));
                }
                assert_eq!(calls, 2);
                if transport {
                    Err(Error::Io(std::io::Error::other("disconnected")))
                } else {
                    Ok(ApiResponse {
                        status: 429,
                        data: json!({"status":"failed"}),
                        retry_after_seconds: Some(16),
                    })
                }
            });
            assert_eq!(calls, 2);
            assert!(!listing.complete && listing.error.is_some());
            assert_eq!(listing.data["speeches"][0]["otid"], "a");
            if !transport {
                assert_eq!(listing.data["pagination"]["retry_after_seconds"], 16);
            }
        }
    }

    #[test]
    fn malformed_pages_cannot_claim_completeness() {
        for data in [
            json!({}),
            json!({"speeches":[{}],"end_of_list":true}),
            json!({"speeches":[{"otid":"a"}],"end_of_list":true}),
            json!({"speeches":[]}),
            json!({"speeches":[],"end_of_list":"true"}),
        ] {
            let listing = collect_pages(true, Some(100.0), 10, |_| {
                Ok(ApiResponse {
                    status: 200,
                    data: data.clone(),
                    retry_after_seconds: None,
                })
            });
            assert!(!listing.complete && listing.error.is_some());
        }
    }
}
