use std::collections::HashMap;

use chrono::TimeZone;
use otter::{ApiResponse, Client, Error};
use serde_json::Value;

/// Render like Python's `json.dumps(data, indent=2)`, including its
/// default ensure_ascii=True escaping of non-ASCII characters.
pub fn print_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("json serializes");
    let mut out = String::with_capacity(pretty.len());
    let mut units = [0u16; 2];
    for c in pretty.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    println!("{out}");
}

/// Python str()-ish rendering of a JSON value: strings bare, missing/null empty.
pub fn value_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Resolve names from the conversation's speaker list for CLI filtering and
/// display. Keep the original response intact for `speeches get --json`.
pub fn transcript_segments(payload: &Value) -> Vec<Value> {
    let speech = &payload["speech"];
    let mut names = HashMap::new();
    if let Some(speakers) = speech["speakers"].as_array() {
        for speaker in speakers {
            let id = value_str(&speaker["speaker_id"]);
            let id = if id.is_empty() {
                value_str(&speaker["id"])
            } else {
                id
            };
            if let Some(name) = speaker["speaker_name"].as_str() {
                if !id.is_empty() && !name.trim().is_empty() {
                    names.insert(id, name);
                }
            }
        }
    }

    let mut segments = speech["transcripts"]
        .as_array()
        .filter(|segments| !segments.is_empty())
        .or_else(|| payload["transcripts"].as_array())
        .cloned()
        .unwrap_or_default();
    for segment in &mut segments {
        // A segment's `id` identifies the segment, not its speaker.
        if let Some(name) = names.get(&value_str(&segment["speaker_id"])) {
            segment["speaker_name"] = Value::String((*name).to_owned());
        }
    }
    segments
}

/// Python truthiness for JSON values.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Render an ApiResponse the way Python's f"{result}" shows the response dict.
pub fn result_repr(result: &ApiResponse) -> String {
    let mut message = format!("{{'status': {}, 'data': {}}}", result.status, result.data);
    if result.status == 429 {
        let wait = match result.retry_after_seconds {
            Some(seconds) => format!("at least {seconds} seconds"),
            None => "60-90 seconds (no server delay supplied)".to_string(),
        };
        message.push_str(&format!(
            "\nRate limited. Stop and wait {wait}, then retry slowly. \
            Separate commands each log in; batch selected tags with repeated -t UUID flags. \
            See otter --help for rate-limit guidance."
        ));
    }
    message
}

/// click.ClickException style: "Error: <msg>" on stderr, exit 1.
pub fn die(message: impl AsRef<str>) -> ! {
    eprintln!("Error: {}", message.as_ref());
    std::process::exit(1)
}

/// Plain stderr message, exit 1 (Python's click.echo(..., err=True) + sys.exit(1)).
pub fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("{}", message.as_ref());
    std::process::exit(1)
}

/// Unwrap a client call, mapping transport errors to "Error: <e>" like the
/// Python CLI's `except OtterAIException` blocks.
pub fn api(result: Result<ApiResponse, Error>) -> ApiResponse {
    match result {
        Ok(response) => response,
        Err(err) => fail(format!("Error: {err}")),
    }
}

/// Epoch seconds -> "Wed Jun 10, 2026 @ 12:41PM ET" (US Eastern), like the
/// Python CLI; falsy -> "", non-numeric -> the raw value.
pub fn format_timestamp(epoch: &Value) -> String {
    if !truthy(epoch) {
        return String::new();
    }
    let secs = match (epoch.as_i64(), epoch.as_f64()) {
        (Some(i), _) => i,
        (None, Some(f)) => f as i64,
        _ => return value_str(epoch),
    };
    match chrono_tz::America::New_York.timestamp_opt(secs, 0) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            dt.format("%a %b %d, %Y @ %I:%M%p ET").to_string()
        }
        chrono::LocalResult::None => value_str(epoch),
    }
}

/// Seconds -> "39m", "1h 5m", "42s", "0s".
pub fn format_duration(seconds: &Value) -> String {
    let secs = seconds
        .as_i64()
        .or_else(|| seconds.as_f64().map(|f| f as i64))
        .unwrap_or(0);
    if secs == 0 {
        return "0s".into();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

/// Resolve a numeric ID or case-insensitive folder name to a folder ID.
/// Err carries the ClickException message so `speeches move --create` can catch it.
pub fn resolve_folder_id(client: &Client, folder_ref: &str) -> Result<String, String> {
    if !folder_ref.is_empty() && folder_ref.chars().all(|c| c.is_ascii_digit()) {
        return Ok(folder_ref.to_string());
    }

    let result = api(client.get_folders());
    if !result.ok() {
        return Err(format!("Failed to list folders: {}", result_repr(&result)));
    }

    if let Some(folders) = result.data["folders"].as_array() {
        for folder in folders {
            if value_str(&folder["folder_name"]).to_lowercase() == folder_ref.to_lowercase() {
                return Ok(value_str(&folder["id"]));
            }
        }
    }

    Err(format!(
        "Folder '{folder_ref}' not found. Use 'otter folders list' to see available folders."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcripts_resolve_metadata_ids_without_changing_the_raw_response() {
        let payload = json!({"speech": {
            "speakers": [
                {"id": 42, "speaker_name": "Alice Example"},
                {"speaker_id": "43", "speaker_name": "Bob Example"},
                {"speaker_name": "Missing ID"},
                {"id": 44, "speaker_name": " "}
            ],
            "transcripts": [
                {"uuid": "a", "speaker_id": "42", "transcript": "hello", "start_offset": 10},
                {"uuid": "b", "speaker_id": 43, "speaker_name": "Old name"},
                {"uuid": "c", "speaker_id": 44, "speaker_name": "Inline name"},
                {"uuid": "d", "speaker_id": 99},
                {"uuid": "e", "id": 42},
                {"uuid": "f", "speaker_id": null}
            ]
        }});
        let original = payload.clone();
        let segments = transcript_segments(&payload);
        let mut expected = payload["speech"]["transcripts"].as_array().unwrap().clone();
        expected[0]["speaker_name"] = json!("Alice Example");
        expected[1]["speaker_name"] = json!("Bob Example");
        assert_eq!(segments, expected);
        assert_eq!(payload, original);
    }

    #[test]
    fn transcripts_support_top_level_and_inline_name_payloads() {
        for nested in [Value::Null, json!([])] {
            let payload = json!({
                "speech": {
                    "speakers": [{"id": "42", "speaker_name": "Alice"}],
                    "transcripts": nested
                },
                "transcripts": [{"speaker_id": 42}, {"speaker_name": "Legacy name"}]
            });
            assert_eq!(
                transcript_segments(&payload),
                vec![
                    json!({"speaker_id": 42, "speaker_name": "Alice"}),
                    json!({"speaker_name": "Legacy name"})
                ]
            );
        }
        assert!(transcript_segments(&json!({})).is_empty());
        let legacy = json!({"transcripts": [{"speaker_name": "Legacy name"}]});
        assert_eq!(
            transcript_segments(&legacy),
            legacy["transcripts"].as_array().unwrap().clone()
        );
    }

    #[test]
    fn timestamp_formats_eastern() {
        // 2024-01-01 00:00:00 UTC == Sun Dec 31, 2023 @ 07:00PM ET
        assert_eq!(
            format_timestamp(&json!(1704067200)),
            "Sun Dec 31, 2023 @ 07:00PM ET"
        );
    }

    #[test]
    fn timestamp_falsy_and_raw() {
        assert_eq!(format_timestamp(&json!(0)), "");
        assert_eq!(format_timestamp(&Value::Null), "");
        assert_eq!(format_timestamp(&json!("2024-01-01")), "2024-01-01");
    }

    #[test]
    fn duration_buckets() {
        assert_eq!(format_duration(&json!(0)), "0s");
        assert_eq!(format_duration(&json!(42)), "42s");
        assert_eq!(format_duration(&json!(2340)), "39m");
        assert_eq!(format_duration(&json!(3900)), "1h 5m");
    }
}
