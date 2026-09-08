use std::collections::HashSet;

use otter::{ApiResponse, Error};
use serde::Serialize;
use serde_json::{json, Value};

use crate::auth::authenticated_client;
use crate::util::{api, fail, print_json, result_repr, value_str};

pub fn list(as_json: bool) {
    let client = authenticated_client();
    let result = api(client.get_speakers());
    if !result.ok() {
        fail(format!("Failed to get speakers: {}", result_repr(&result)));
    }

    if as_json {
        print_json(&result.data);
        return;
    }

    let speakers = result.data["speakers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if speakers.is_empty() {
        println!("No speakers found.");
        return;
    }

    println!("Found {} speakers:\n", speakers.len());
    for speaker in &speakers {
        let name = match value_str(&speaker["speaker_name"]) {
            n if n.is_empty() => "Unknown".to_string(),
            n => n,
        };
        println!("  {}  {name}", speaker_id_of(speaker));
    }
}

pub fn create(name: String) {
    let client = authenticated_client();
    let result = api(client.create_speaker(&name));
    if !result.ok() {
        fail(format!(
            "Failed to create speaker: {}",
            result_repr(&result)
        ));
    }
    println!("Speaker '{name}' created.");
    print_json(&result.data);
}

pub fn tag(
    speech_id: String,
    speaker_id: String,
    transcript_uuid: Vec<String>,
    tag_all: bool,
    as_json: bool,
) {
    let client = authenticated_client();

    let speakers_result = api(client.get_speakers());
    if !speakers_result.ok() {
        fail(format!(
            "Failed to get speakers: {}",
            result_repr(&speakers_result)
        ));
    }

    let mut speaker_name = None;
    if let Some(speakers) = speakers_result.data["speakers"].as_array() {
        for speaker in speakers {
            if speaker_id_of(speaker) == speaker_id {
                speaker_name = Some(value_str(&speaker["speaker_name"]));
                break;
            }
        }
    }
    let Some(speaker_name) = speaker_name else {
        fail(format!("Speaker ID {speaker_id} not found."));
    };

    let speech_result = api(client.get_speech(&speech_id));
    if !speech_result.ok() {
        fail(format!(
            "Failed to get speech: {}",
            result_repr(&speech_result)
        ));
    }

    let transcripts = speech_result.data["speech"]["transcripts"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    if transcript_uuid.is_empty() && !tag_all {
        // List available transcript segments.
        if as_json {
            let segments: Vec<Value> = transcripts
                .iter()
                .map(|t| {
                    json!({
                        "uuid": value_str(&t["uuid"]),
                        "speaker_id": t["speaker_id"],
                        "speaker_name": if t["speaker_name"].is_string() { t["speaker_name"].clone() } else { json!("Untagged") },
                        "text_preview": chars_prefix(&value_str(&t["transcript"]), 80),
                    })
                })
                .collect();
            print_json(&Value::Array(segments));
        } else {
            println!("Available transcript segments in {speech_id}:\n");
            for t in &transcripts {
                let current = match value_str(&t["speaker_name"]) {
                    s if s.is_empty() => "Untagged".to_string(),
                    s => s,
                };
                println!("  UUID: {}", value_str(&t["uuid"]));
                println!("  Speaker: {current}");
                println!(
                    "  Text: {}...",
                    chars_prefix(&value_str(&t["transcript"]), 60)
                );
                println!();
            }
            println!("Repeat -t <uuid> to tag selected segments in one session. --all labels EVERY segment.");
        }
        return;
    }

    let segments_to_tag = select_segments(&transcripts, &transcript_uuid, tag_all)
        .unwrap_or_else(|message| fail(message));
    // The same authenticated client handles all selected segments. Do not call
    // authenticated_client() inside this loop: separate logins cause HTTP 429s.
    let report = tag_segments(&segments_to_tag, |uuid| {
        client.set_transcript_speaker(&speech_id, uuid, &speaker_id, &speaker_name, false)
    });
    if as_json {
        let mut result = serde_json::to_value(&report).expect("report serializes");
        result["status"] = json!(if report.error.is_none() {
            "OK"
        } else {
            "failed"
        });
        result["speech_otid"] = json!(speech_id);
        result["speaker_id"] = json!(speaker_id);
        result["speaker_name"] = json!(speaker_name);
        print_json(&result);
    } else {
        println!(
            "Tagged {}/{} segments as '{speaker_name}'",
            report.tagged_uuids.len(),
            segments_to_tag.len()
        );
        if report.error.is_some() {
            eprintln!("Saved UUIDs: {}", report.tagged_uuids.join(","));
            eprintln!("Unattempted UUIDs: {}", report.unattempted_uuids.join(","));
        }
    }
    if let Some(error) = report.error {
        fail(format!("Stopped at segment {}: {error}\nReload the failed segment before retrying an uncertain write.", report.failed_uuid.unwrap_or_default()));
    }
}

/// Validate the complete selection before saving anything, and keep request order.
fn select_segments(
    transcripts: &[Value],
    requested: &[String],
    tag_all: bool,
) -> Result<Vec<String>, String> {
    if tag_all && !requested.is_empty() {
        return Err("--all cannot be combined with selected transcript UUIDs".into());
    }
    let available: Vec<String> = transcripts
        .iter()
        .map(|t| value_str(&t["uuid"]))
        .filter(|uuid| !uuid.is_empty())
        .collect();
    let candidates = if tag_all { &available } else { requested };
    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for uuid in candidates {
        if !available.contains(uuid) {
            return Err(format!(
                "Transcript UUID {uuid:?} is not in this conversation; no tags were saved."
            ));
        }
        if seen.insert(uuid) {
            selected.push(uuid.clone());
        }
    }
    Ok(selected)
}

#[derive(Serialize)]
struct TagReport {
    tagged_uuids: Vec<String>,
    failed_uuid: Option<String>,
    unattempted_uuids: Vec<String>,
    error: Option<String>,
    retry_after_seconds: Option<u64>,
}

/// Stop at the first failure. In particular, do not continue hammering a
/// rate-limited endpoint or automatically replay a potentially saved mutation.
fn tag_segments(
    segments: &[String],
    mut tag: impl FnMut(&str) -> Result<ApiResponse, Error>,
) -> TagReport {
    let mut report = TagReport {
        tagged_uuids: Vec::new(),
        failed_uuid: None,
        unattempted_uuids: Vec::new(),
        error: None,
        retry_after_seconds: None,
    };
    for (index, uuid) in segments.iter().enumerate() {
        match tag(uuid) {
            Ok(result) if result.ok() && result.data["status"] != "failed" => {
                report.tagged_uuids.push(uuid.clone());
                continue;
            }
            Ok(result) => {
                report.retry_after_seconds = result.retry_after_seconds;
                report.error = Some(result_repr(&result));
            }
            Err(error) => report.error = Some(error.to_string()),
        }
        report.failed_uuid = Some(uuid.clone());
        report.unattempted_uuids = segments[index + 1..].to_vec();
        break;
    }
    report
}

fn chars_prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// The API now returns "id"; older payloads used "speaker_id".
fn speaker_id_of(speaker: &Value) -> String {
    match value_str(&speaker["speaker_id"]) {
        id if id.is_empty() => value_str(&speaker["id"]),
        id => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn response(status: u16, data: Value, retry_after_seconds: Option<u64>) -> ApiResponse {
        ApiResponse {
            status,
            data,
            retry_after_seconds,
        }
    }

    #[test]
    fn selection_preserves_requested_order_deduplicates_and_excludes_other_speakers() {
        let transcripts = vec![
            json!({"uuid": "a"}),
            json!({"uuid": "b"}),
            json!({"uuid": "c"}),
        ];
        assert_eq!(
            select_segments(&transcripts, &ids(&["c", "a", "c"]), false).unwrap(),
            ids(&["c", "a"])
        );
        assert!(select_segments(&transcripts, &ids(&["a", "missing"]), false).is_err());
        assert!(select_segments(&transcripts, &ids(&["a"]), true).is_err());
        assert_eq!(
            select_segments(&transcripts, &[], true).unwrap(),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn batch_stops_on_rate_limit_and_reports_exact_progress() {
        let mut calls = Vec::new();
        let report = tag_segments(&ids(&["a", "b", "c"]), |uuid| {
            calls.push(uuid.to_owned());
            Ok(if uuid == "a" {
                response(200, json!({"status": "OK"}), None)
            } else {
                response(
                    429,
                    json!({"status": "failed", "retry_after": 16}),
                    Some(16),
                )
            })
        });
        assert_eq!(calls, ids(&["a", "b"]));
        assert_eq!(report.tagged_uuids, ids(&["a"]));
        assert_eq!(report.failed_uuid.as_deref(), Some("b"));
        assert_eq!(report.unattempted_uuids, ids(&["c"]));
        assert_eq!(report.retry_after_seconds, Some(16));
        assert!(report
            .error
            .as_deref()
            .unwrap()
            .contains("at least 16 seconds"));
    }

    #[test]
    fn uncertain_transport_failure_is_not_retried_or_reported_as_saved() {
        let mut calls = 0;
        let report = tag_segments(&ids(&["a", "b"]), |_| {
            calls += 1;
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset").into())
        });
        assert_eq!(calls, 1);
        assert!(report.tagged_uuids.is_empty());
        assert_eq!(report.failed_uuid.as_deref(), Some("a"));
        assert_eq!(report.unattempted_uuids, ids(&["b"]));
        assert!(report.error.is_some());
    }

    #[test]
    fn api_failure_with_http_200_still_stops_the_batch() {
        let report = tag_segments(&ids(&["a", "b"]), |_| {
            Ok(response(
                200,
                json!({"status": "failed", "message": "permission denied"}),
                None,
            ))
        });
        assert!(report.tagged_uuids.is_empty());
        assert_eq!(report.failed_uuid.as_deref(), Some("a"));
        assert_eq!(report.unattempted_uuids, ids(&["b"]));
    }

    #[test]
    fn successful_batch_has_machine_readable_results() {
        let report = tag_segments(&ids(&["a", "b"]), |_| {
            Ok(response(200, json!({"status": "OK"}), None))
        });
        let data = serde_json::to_value(&report).unwrap();
        assert_eq!(data["tagged_uuids"], json!(["a", "b"]));
        assert_eq!(data["unattempted_uuids"], json!([]));
        assert!(data["failed_uuid"].is_null());
        assert!(data["error"].is_null());
    }
}
