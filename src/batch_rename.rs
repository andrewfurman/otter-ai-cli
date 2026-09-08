use std::collections::HashSet;
use std::path::PathBuf;

use otter::{ApiResponse, Error};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::authenticated_client;
use crate::util::{fail, print_json, result_repr};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Change {
    otid: String,
    old_title: Value,
    new_title: String,
}

fn parse_plan(text: &str) -> Result<Vec<Change>, String> {
    let plan: Vec<Change> =
        serde_json::from_str(text).map_err(|error| format!("Invalid rename plan: {error}"))?;
    if plan.is_empty() {
        return Err("Rename plan is empty.".into());
    }
    let mut seen = HashSet::new();
    for (index, change) in plan.iter().enumerate() {
        if change.otid.is_empty() || change.otid.chars().any(char::is_whitespace) {
            return Err(format!(
                "Plan entry {} needs a nonempty OTID without whitespace.",
                index + 1
            ));
        }
        if !seen.insert(&change.otid) {
            return Err(format!("Duplicate OTID {} in rename plan.", change.otid));
        }
        if !change.old_title.is_null() && !change.old_title.is_string() {
            return Err(format!(
                "old_title for {} must be a string or null.",
                change.otid
            ));
        }
        if change.new_title.trim().is_empty() {
            return Err(format!("new_title for {} must not be blank.", change.otid));
        }
    }
    Ok(plan)
}

#[derive(Default, Serialize)]
struct Report {
    saved_otids: Vec<String>,
    unchanged_otids: Vec<String>,
    failed_otid: Option<String>,
    unattempted_otids: Vec<String>,
    error: Option<String>,
    retry_after_seconds: Option<u64>,
    /// A rename was sent but has not been verified as saved.
    unconfirmed_write: bool,
}

struct Failure {
    message: String,
    retry_after_seconds: Option<u64>,
    unconfirmed_write: bool,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after_seconds: None,
            unconfirmed_write: false,
        }
    }
}

fn checked(result: Result<ApiResponse, Error>) -> Result<Value, Failure> {
    match result {
        Ok(response) if response.ok() => Ok(response.data),
        Ok(response) => Err(Failure {
            message: result_repr(&response),
            retry_after_seconds: response.retry_after_seconds,
            unconfirmed_write: false,
        }),
        Err(error) => Err(Failure::new(error.to_string())),
    }
}

fn read_title(result: Result<ApiResponse, Error>, otid: &str) -> Result<Value, Failure> {
    let data = checked(result)?;
    let speech = &data["speech"];
    if speech["otid"].as_str() != Some(otid) {
        return Err(Failure::new(
            "Detail response did not identify the requested recording.",
        ));
    }
    match speech.get("title") {
        Some(title) if title.is_string() || title.is_null() => Ok(title.clone()),
        _ => Err(Failure::new(
            "Detail response did not include a valid title.",
        )),
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Saved,
    Unchanged,
}

fn apply_change(
    change: &Change,
    get: &mut impl FnMut(&str) -> Result<ApiResponse, Error>,
    rename: &mut impl FnMut(&str, &str) -> Result<ApiResponse, Error>,
) -> Result<Outcome, Failure> {
    let current = read_title(get(&change.otid), &change.otid)?;
    if current.as_str() == Some(&change.new_title) {
        return Ok(Outcome::Unchanged);
    }
    if current != change.old_title {
        return Err(Failure::new("Current title differs from old_title; no rename was sent for this recording. Refresh the plan before retrying."));
    }
    // From this point onward, any error may leave a saved but unverified write.
    let mut verify = || -> Result<(), Failure> {
        checked(rename(&change.otid, &change.new_title))?;
        let actual = read_title(get(&change.otid), &change.otid)?;
        if actual.as_str() != Some(&change.new_title) {
            return Err(Failure::new(
                "Title did not match new_title after the rename.",
            ));
        }
        Ok(())
    };
    verify().map_err(|mut failure| {
        failure.unconfirmed_write = true;
        failure
    })?;
    Ok(Outcome::Saved)
}

fn execute(
    plan: &[Change],
    mut get: impl FnMut(&str) -> Result<ApiResponse, Error>,
    mut rename: impl FnMut(&str, &str) -> Result<ApiResponse, Error>,
    mut progress: impl FnMut(usize, &str, &str),
) -> Report {
    let mut report = Report::default();
    for (index, change) in plan.iter().enumerate() {
        match apply_change(change, &mut get, &mut rename) {
            Ok(Outcome::Saved) => {
                report.saved_otids.push(change.otid.clone());
                progress(index + 1, &change.otid, "saved and verified");
            }
            Ok(Outcome::Unchanged) => {
                report.unchanged_otids.push(change.otid.clone());
                progress(index + 1, &change.otid, "already correct");
            }
            Err(error) => {
                report.failed_otid = Some(change.otid.clone());
                report.unattempted_otids = plan[index + 1..]
                    .iter()
                    .map(|change| change.otid.clone())
                    .collect();
                report.error = Some(error.message);
                report.retry_after_seconds = error.retry_after_seconds;
                report.unconfirmed_write = error.unconfirmed_write;
                break;
            }
        }
    }
    report
}

pub fn run(file: PathBuf, dry_run: bool, as_json: bool) {
    // Validate the entire file before authentication or any mutations.
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| fail(format!("Could not read rename plan: {error}")));
    let plan = parse_plan(&text).unwrap_or_else(|error| fail(error));
    if dry_run {
        if as_json {
            print_json(
                &json!({"status": "preview", "dry_run": true, "count": plan.len(), "changes": plan}),
            );
        } else {
            println!(
                "Preview: {} recordings (no authentication or changes)",
                plan.len()
            );
            for change in &plan {
                println!(
                    "{}: {} -> {}",
                    change.otid,
                    change.old_title,
                    json!(change.new_title)
                );
            }
        }
        return;
    }
    let client = authenticated_client();
    let report = execute(
        &plan,
        |otid| client.get_speech(otid),
        |otid, title| client.set_speech_title(otid, title),
        |index, otid, status| eprintln!("[{index}/{}] {otid}: {status}", plan.len()),
    );
    if as_json {
        let mut data = serde_json::to_value(&report).expect("report serializes");
        data["status"] = json!(if report.error.is_none() {
            "OK"
        } else {
            "failed"
        });
        data["dry_run"] = json!(false);
        data["total"] = json!(plan.len());
        print_json(&data);
    } else {
        println!(
            "Saved and verified: {}. Already correct: {}. Unattempted: {}.",
            report.saved_otids.len(),
            report.unchanged_otids.len(),
            report.unattempted_otids.len()
        );
        if report.error.is_some() {
            println!(
                "Saved OTIDs: {}\nUnchanged OTIDs: {}\nUnattempted OTIDs: {}",
                report.saved_otids.join(","),
                report.unchanged_otids.join(","),
                report.unattempted_otids.join(",")
            );
        }
    }
    if let Some(error) = report.error {
        fail(format!("Stopped at {}: {error}\nReload the failed recording before retrying. Saved recordings remain saved; rerunning a plan skips already-correct titles.", report.failed_otid.unwrap_or_default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn ok(data: Value) -> Result<ApiResponse, Error> {
        Ok(ApiResponse {
            status: 200,
            data,
            retry_after_seconds: None,
        })
    }
    fn detail(otid: &str, title: &str) -> Result<ApiResponse, Error> {
        ok(json!({"status":"OK", "speech":{"otid":otid,"title":title}}))
    }
    fn plan() -> Vec<Change> {
        parse_plan(r#"[{"otid":"a","old_title":"old","new_title":"new"},{"otid":"b","old_title":"old","new_title":"new"},{"otid":"c","old_title":"old","new_title":"new"}]"#).unwrap()
    }

    #[test]
    fn entire_plan_requires_unique_ids_and_explicit_valid_titles() {
        for input in [
            "{",
            "{}",
            "[]",
            r#"[{"otid":"a","new_title":"new"}]"#,
            r#"[{"otid":"a","old_title":42,"new_title":"new"}]"#,
            r#"[{"otid":"a","old_title":null,"new_title":"  "}]"#,
            r#"[{"otid":"a b","old_title":null,"new_title":"new"}]"#,
            r#"[{"otid":"a","old_title":null,"new_title":"new","typo":1}]"#,
            r#"[{"otid":"a","old_title":null,"new_title":"new"},{"otid":"a","old_title":null,"new_title":"new"}]"#,
        ] {
            assert!(parse_plan(input).is_err(), "accepted {input}");
        }
        let plan =
            parse_plan(r#"[{"otid":"a","old_title":null,"new_title":"Café & QA"}]"#).unwrap();
        assert!(plan[0].old_title.is_null());
        assert_eq!(plan[0].new_title, "Café & QA");
    }

    #[test]
    fn saves_are_read_back_and_completed_entries_are_skipped_on_resume() {
        let titles = RefCell::new(std::collections::HashMap::from([
            ("a".to_string(), "old".to_string()),
            ("b".to_string(), "new".to_string()),
            ("c".to_string(), "old".to_string()),
        ]));
        let calls = RefCell::new(Vec::new());
        let report = execute(
            &plan(),
            |otid| {
                calls.borrow_mut().push(format!("get:{otid}"));
                detail(otid, &titles.borrow()[otid])
            },
            |otid, title| {
                calls.borrow_mut().push(format!("rename:{otid}"));
                titles
                    .borrow_mut()
                    .insert(otid.to_owned(), title.to_owned());
                ok(json!({"status":"OK"}))
            },
            |_, _, _| {},
        );
        assert_eq!(
            *calls.borrow(),
            ["get:a", "rename:a", "get:a", "get:b", "get:c", "rename:c", "get:c"]
        );
        assert_eq!(report.saved_otids, ["a", "c"]);
        assert_eq!(report.unchanged_otids, ["b"]);
        assert!(report.error.is_none());
        let resumed = execute(
            &plan(),
            |otid| detail(otid, &titles.borrow()[otid]),
            |_, _| panic!("resume must not rename"),
            |_, _, _| {},
        );
        assert_eq!(resumed.unchanged_otids, ["a", "b", "c"]);
    }

    #[test]
    fn stale_titles_stop_before_writes_and_leave_remaining_entries_unattempted() {
        let mut gets = 0;
        let report = execute(
            &plan(),
            |otid| {
                gets += 1;
                detail(otid, "changed by someone else")
            },
            |_, _| panic!("must not overwrite"),
            |_, _, _| {},
        );
        assert_eq!(gets, 1);
        assert_eq!(report.failed_otid.as_deref(), Some("a"));
        assert_eq!(report.unattempted_otids, ["b", "c"]);
        assert!(!report.unconfirmed_write);
    }

    #[test]
    fn partial_batch_stops_on_429_and_preserves_progress_and_delay() {
        let mut reads = 0;
        let mut writes = 0;
        let report = execute(
            &plan(),
            |otid| {
                reads += 1;
                detail(otid, if reads == 2 { "new" } else { "old" })
            },
            |_, _| {
                writes += 1;
                if writes == 1 {
                    ok(json!({"status":"OK"}))
                } else {
                    Ok(ApiResponse {
                        status: 429,
                        data: json!({"status":"failed"}),
                        retry_after_seconds: Some(16),
                    })
                }
            },
            |_, _, _| {},
        );
        assert_eq!(writes, 2);
        assert_eq!(reads, 3);
        assert_eq!(report.saved_otids, ["a"]);
        assert_eq!(report.failed_otid.as_deref(), Some("b"));
        assert_eq!(report.unattempted_otids, ["c"]);
        assert_eq!(report.retry_after_seconds, Some(16));
        assert!(report.unconfirmed_write);
    }

    #[test]
    fn transport_failure_or_failed_verification_never_counts_as_saved() {
        for mode in [
            "disconnect",
            "mismatch",
            "bad_detail",
            "read_429",
            "api_failed",
        ] {
            let mut reads = 0;
            let mut writes = 0;
            let report = execute(
                &plan(),
                |otid| {
                    reads += 1;
                    if reads == 1 {
                        return detail(otid, "old");
                    }
                    match mode {
                        "bad_detail" => ok(json!({"speech":{"otid":"other","title":"new"}})),
                        "read_429" => Ok(ApiResponse {
                            status: 429,
                            data: json!({"status":"failed"}),
                            retry_after_seconds: Some(30),
                        }),
                        _ => detail(otid, "old"),
                    }
                },
                |_, _| {
                    writes += 1;
                    match mode {
                        "disconnect" => Err(Error::Io(std::io::Error::other("connection lost"))),
                        "api_failed" => ok(json!({"status":"failed"})),
                        _ => ok(json!({"status":"OK"})),
                    }
                },
                |_, _, _| {},
            );
            assert_eq!(writes, 1);
            assert!(report.saved_otids.is_empty());
            assert_eq!(report.failed_otid.as_deref(), Some("a"));
            assert_eq!(report.unattempted_otids, ["b", "c"]);
            assert!(report.unconfirmed_write);
            if mode == "read_429" {
                assert_eq!(report.retry_after_seconds, Some(30));
            }
        }
    }

    #[test]
    fn null_titles_are_supported_but_missing_or_unrelated_details_fail() {
        for data in [
            json!({}),
            json!({"speech":{"otid":"a"}}),
            json!({"speech":{"otid":"other","title":"old"}}),
            json!({"speech":{"otid":"a","title":42}}),
        ] {
            assert!(read_title(ok(data), "a").is_err());
        }
        assert_eq!(
            read_title(ok(json!({"speech":{"otid":"a","title":null}})), "a").ok(),
            Some(Value::Null)
        );
    }
}
