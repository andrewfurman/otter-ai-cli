use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crate::{
    pagination,
    util::{fail, transcript_segments},
};

fn save(path: &Path, data: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(data)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn archive_metadata(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| {
                    let key = key.to_ascii_lowercase();
                    !["token", "jwt", "password", "cookie", "secret", "session"]
                        .iter()
                        .any(|part| key.contains(part))
                        && !key.ends_with("url")
                        && !key.ends_with("urls")
                })
                .map(|(key, value)| (key.clone(), archive_metadata(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(archive_metadata).collect()),
        _ => value.clone(),
    }
}

fn timestamp(milliseconds: &Value) -> Value {
    let Some(ms) = milliseconds.as_f64().filter(|n| n.is_finite() && *n >= 0.0) else {
        return Value::Null;
    };
    let ms = ms.round() as u64;
    json!(format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    ))
}

fn iso_time(value: &Value) -> Value {
    value
        .as_i64()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|date| json!(date.to_rfc3339()))
        .unwrap_or(Value::Null)
}

fn recorded_time(speech: &Value) -> (Option<i64>, &'static str) {
    if let Some(time) = speech["title"]
        .as_str()
        .and_then(crate::search::parse_title_time_et)
    {
        return (Some(time), "title_date_et");
    }
    for key in ["displayed_start_time", "start_time", "created_at"] {
        if let Some(time) = speech[key].as_i64().filter(|time| *time > 0) {
            return (Some(time), key);
        }
    }
    (None, "unknown")
}

fn document(raw: Value) -> Value {
    let speech = &raw["speech"];
    let (recorded_at, date_source) = recorded_time(speech);
    let segments: Vec<Value> = transcript_segments(&raw)
        .into_iter()
        .map(|segment| {
            json!({"uuid": segment["uuid"], "speaker_id": segment["speaker_id"],
            "speaker_name": segment["speaker_name"], "text": segment["transcript"],
            "start_ms": segment["start_offset"], "end_ms": segment["end_offset"],
            "start_seconds": segment["start_offset"].as_f64().map(|n| n / 1000.0),
            "end_seconds": segment["end_offset"].as_f64().map(|n| n / 1000.0),
            "start_timestamp": timestamp(&segment["start_offset"]),
            "end_timestamp": timestamp(&segment["end_offset"]),
            "raw_segment": segment})
        })
        .collect();
    let text = segments
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "schema_version": 1,
        "recorded_at_unix_seconds": recorded_at, "recorded_at_iso_utc": iso_time(&json!(recorded_at)), "recorded_at_source": date_source,
        "exported_at_unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "otid": speech["otid"], "title": speech["title"],
        "created_at": speech["created_at"], "duration_seconds": speech["duration"],
        "recording_start_time": speech["start_time"], "displayed_start_time": speech["displayed_start_time"],
        "recording_start_iso_utc": iso_time(&speech["start_time"]),
        "displayed_start_iso_utc": iso_time(&speech["displayed_start_time"]),
        "created_at_iso_utc": iso_time(&speech["created_at"]),
        "timezone": speech["timezone"], "timestamp_unit": "milliseconds from start of recording",
        "speakers": speech["speakers"], "segments": segments,
        "text": text, "raw_response": archive_metadata(&raw)
    })
}

fn record_limit(
    stage: &str,
    retry_after: Option<u64>,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let event = json!({"observed_at_unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "stage": stage, "http_status": 429, "retry_after_seconds": retry_after});
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output.join("rate-limits.jsonl"))?;
    serde_json::to_writer(&mut file, &event)?;
    writeln!(file)?;
    Ok(())
}

fn check_response(
    response: &otter::ApiResponse,
    stage: &str,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if response.status == 429 {
        record_limit(stage, response.retry_after_seconds, output)?;
    }
    if !response.ok() {
        return Err(format!(
            "{stage}: HTTP {}; retry_after_seconds={:?}",
            response.status, response.retry_after_seconds
        )
        .into());
    }
    Ok(())
}

pub fn run(output: PathBuf, source: String, delay: u64, max_new: u32) {
    if let Err(error) = export(&output, &source, delay, max_new) {
        fail(error.to_string());
    }
}

fn export(
    output: &Path,
    source: &str,
    delay: u64,
    max_new: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(output.join("conversations"))?;
    let (Some(username), Some(password)) = otter::config::load_credentials() else {
        return Err("Run otter login first".into());
    };
    let mut client = otter::Client::new()?;
    let login = client.login(&username, &password)?;
    check_response(&login, "login", output)?;
    let cached = output.join("listing.json");
    let previous: Option<Value> = fs::read(&output.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let listing = if cached.exists()
        && previous
            .as_ref()
            .is_some_and(|m| m["source"] == source && m["listing_complete"] == true)
    {
        let data: Value = serde_json::from_slice(&fs::read(&cached)?)?;
        pagination::Listing {
            data,
            complete: true,
            error: None,
        }
    } else {
        let mut pages = 0;
        let listing = pagination::collect_pages(true, None, 1000, |cursor| {
            thread::sleep(Duration::from_secs(delay));
            let response = client.get_speeches_page("0", 100, source, cursor)?;
            pages += 1;
            eprintln!("Listing page {pages}: HTTP {}", response.status);
            // Record rate limits without retaining response bodies or account identifiers.
            if response.status == 429 {
                let _ = record_limit("listing", response.retry_after_seconds, output);
            }
            Ok(response)
        });
        save(&cached, &archive_metadata(&listing.data))?;
        listing
    };
    let speeches = listing.data["speeches"]
        .as_array()
        .ok_or("Listing missing speeches")?;
    let mut speeches = speeches.clone();
    speeches.sort_by(|a, b| {
        recorded_time(b)
            .0
            .cmp(&recorded_time(a).0)
            .then_with(|| a["otid"].as_str().cmp(&b["otid"].as_str()))
    });
    let mut manifest = json!({"schema_version": 1, "source": source, "listing_complete": listing.complete,
        "total": speeches.len(), "order": "recorded_at_desc", "exported": 0, "complete": false, "error": listing.error});
    save(&output.join("manifest.json"), &manifest)?;
    if !listing.complete {
        return Err("Listing incomplete; see listing.json. No conversations fetched.".into());
    }
    let mut index = fs::File::create(output.join("index.jsonl.tmp"))?;
    let mut segment_index = fs::File::create(output.join("segments.jsonl.tmp"))?;
    let mut fetched = 0;
    let mut exported = 0;
    for speech in &speeches {
        let id = speech["otid"].as_str().ok_or("Missing OTID")?;
        // Encode every byte to avoid filesystem traversal and name collisions.
        let name = id.bytes().map(|b| format!("{b:02x}")).collect::<String>();
        let relative = format!("conversations/{name}.json");
        let path = output.join(&relative);
        if !path.exists() && fetched >= max_new {
            continue;
        }
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            if path.exists() {
                let doc: Value = serde_json::from_slice(&fs::read(&path)?)?;
                if doc["otid"] != id
                    || doc["schema_version"] != 1
                    || !doc["segments"].is_array()
                    || !doc["text"].is_string()
                {
                    return Err(
                        "Existing export failed validation; move it aside before retrying".into(),
                    );
                }
                return Ok(doc);
            }
            thread::sleep(Duration::from_secs(delay));
            let response = client.get_speech(id)?;
            check_response(&response, "speech", output)?;
            fetched += 1;
            let doc = document(response.data);
            save(&path, &doc)?;
            Ok(doc)
        })();
        let doc = match result {
            Ok(doc) => doc,
            Err(error) => {
                manifest["error"] = json!(format!("{id}: {error}"));
                save(&output.join("manifest.json"), &manifest)?;
                return Err(error);
            }
        };
        let row = json!({"otid": id, "title": doc["title"], "created_at": doc["created_at"],
            "duration_seconds": doc["duration_seconds"], "speakers": doc["speakers"],
            "recorded_at_iso_utc": doc["recorded_at_iso_utc"], "recording_start_iso_utc": doc["recording_start_iso_utc"], "created_at_iso_utc": doc["created_at_iso_utc"],
            "text": doc["text"], "path": relative});
        serde_json::to_writer(&mut index, &row)?;
        writeln!(index)?;
        for segment in doc["segments"].as_array().ok_or("Missing segments")? {
            let mut row = segment.clone();
            row.as_object_mut().unwrap().remove("raw_segment");
            row["otid"] = json!(id);
            row["title"] = doc["title"].clone();
            row["recorded_at_iso_utc"] = doc["recorded_at_iso_utc"].clone();
            row["recording_start_iso_utc"] = doc["recording_start_iso_utc"].clone();
            row["path"] = json!(relative);
            serde_json::to_writer(&mut segment_index, &row)?;
            writeln!(segment_index)?;
        }
        exported += 1;
        manifest["exported"] = json!(exported);
        save(&output.join("manifest.json"), &manifest)?;
        eprintln!(
            "Exported {}/{} ({fetched} new this run)",
            exported,
            speeches.len()
        );
    }
    segment_index.sync_all()?;
    fs::rename(
        output.join("segments.jsonl.tmp"),
        output.join("segments.jsonl"),
    )?;
    index.sync_all()?;
    fs::rename(output.join("index.jsonl.tmp"), output.join("index.jsonl"))?;
    manifest["complete"] = json!(exported == speeches.len());
    save(&output.join("manifest.json"), &manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_order_uses_title_dates_before_upload_dates() {
        let imported =
            json!({"title": "Call on Mon Sep 21st 2026 @ 7:37am ET", "created_at": 2_000_000_000});
        let (time, source) = recorded_time(&imported);
        assert_eq!(source, "title_date_et");
        assert!(time.unwrap() < 2_000_000_000);
        assert_eq!(
            recorded_time(&json!({"displayed_start_time": 123, "created_at": 999})),
            (Some(123), "displayed_start_time")
        );
        assert_eq!(recorded_time(&json!({})), (None, "unknown"));
    }

    #[test]
    fn strips_access_metadata_and_handles_missing_timing() {
        let value =
            json!({"speech": {"pubsub_jwt": "secret", "audio_url": "signed", "title": "Title"}});
        assert_eq!(
            archive_metadata(&value),
            json!({"speech": {"title": "Title"}})
        );
        assert_eq!(iso_time(&json!(0)), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_time(&Value::Null), Value::Null);
        assert_eq!(timestamp(&Value::Null), Value::Null);
        assert_eq!(timestamp(&json!(3_723_456)), "01:02:03.456");
    }

    #[test]
    fn preserves_raw_and_resolves_speakers_with_numeric_ids() {
        let raw = json!({"speech": {"otid": "abc", "speakers": [
            {"speaker_id": "2", "speaker_name": "Alice"}], "transcripts": [
            {"speaker_id": 2, "transcript": "Hello", "start_offset": 12}]}});
        let doc = document(raw.clone());
        assert_eq!(doc["raw_response"], raw);
        assert_eq!(doc["segments"][0]["speaker_name"], "Alice");
        assert_eq!(doc["segments"][0]["start_ms"], 12);
        assert_eq!(doc["text"], "Hello");
        assert_eq!(doc["segments"][0]["start_seconds"], 0.012);
        assert_eq!(doc["segments"][0]["start_timestamp"], "00:00:00.012");
    }
}
