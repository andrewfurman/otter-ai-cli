use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

fn conversation_path(output: &Path, id: &str) -> PathBuf {
    let name = id.bytes().map(|b| format!("{b:02x}")).collect::<String>();
    let legacy = output.join(format!("conversations/{name}.json"));
    let compressed = output.join(format!("conversations/{name}.json.gz"));
    if legacy.exists() && !compressed.exists() {
        legacy
    } else {
        compressed
    }
}

fn read_document(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = if path.extension().is_some_and(|extension| extension == "gz") {
        let result = Command::new("gzip")
            .arg("-dc")
            .arg("--")
            .arg(path)
            .output()?;
        if !result.status.success() {
            return Err("Cannot decompress saved conversation".into());
        }
        result.stdout
    } else {
        fs::read(path)?
    };
    Ok(serde_json::from_slice(&bytes)?)
}

fn save_document(path: &Path, doc: &Value) -> Result<(), Box<dyn std::error::Error>> {
    if path.extension().is_none_or(|extension| extension != "gz") {
        return save(path, doc);
    }
    let temporary = path.with_extension("gz.tmp");
    let file = fs::File::create(&temporary)?;
    let mut child = Command::new("gzip")
        .args(["-c", "-3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(file))
        .spawn()?;
    let bytes = serde_json::to_vec(doc)?;
    let mut input = child.stdin.take().ok_or("Missing compressor input")?;
    let written = input.write_all(&bytes);
    drop(input);
    let status = child.wait()?;
    written?;
    if !status.success() {
        return Err("Conversation compression failed".into());
    }
    fs::rename(temporary, path)?;
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

fn merge_listing(current: &mut Value, prior: &Value) {
    let mut seen = std::collections::HashSet::new();
    let mut speeches = Vec::new();
    for data in [prior, &*current] {
        if let Some(items) = data["speeches"].as_array() {
            for speech in items {
                if let Some(id) = speech["otid"].as_str() {
                    if seen.insert(id.to_owned()) {
                        speeches.push(speech.clone());
                    }
                }
            }
        }
    }
    if current["last_load_ts"].as_u64().is_none() {
        current["last_load_ts"] = prior["last_load_ts"].clone();
    }
    current["pagination"]["unique_fetched"] = json!(speeches.len());
    current["speeches"] = json!(speeches);
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
    let unavailable_path = output.join("unavailable.json");
    let mut unavailable: Value = if unavailable_path.exists() {
        serde_json::from_slice(&fs::read(&unavailable_path)?)?
    } else {
        json!({})
    };
    if !unavailable.is_object() {
        return Err("Invalid unavailable.json".into());
    }
    let prior = if cached.exists() && previous.as_ref().is_some_and(|m| m["source"] == source) {
        Some(serde_json::from_slice::<Value>(&fs::read(&cached)?)?)
    } else {
        None
    };
    let pending = prior
        .as_ref()
        .and_then(|data| data["speeches"].as_array())
        .map(|speeches| {
            speeches
                .iter()
                .filter(|speech| {
                    speech["otid"].as_str().is_some_and(|id| {
                        !conversation_path(output, id).exists() && unavailable.get(id).is_none()
                    })
                })
                .count()
        })
        .unwrap_or(0);
    let prior_complete = prior
        .as_ref()
        .is_some_and(|data| data["pagination"]["complete"] == true);
    let listing = if prior_complete || pending >= max_new as usize {
        pagination::Listing {
            data: prior.unwrap(),
            complete: prior_complete,
            error: None,
        }
    } else {
        let resume = prior
            .as_ref()
            .and_then(|data| data["last_load_ts"].as_u64());
        let mut pages = 0;
        // Bound discovery too; resume older pages on a later batch.
        let mut listing = pagination::collect_pages(true, None, 3, |cursor| {
            thread::sleep(Duration::from_secs(delay));
            let response = client.get_speeches_page("0", 100, source, cursor.or(resume))?;
            pages += 1;
            eprintln!("Listing page {pages}: HTTP {}", response.status);
            if response.status == 429 {
                let _ = record_limit("listing", response.retry_after_seconds, output);
            }
            Ok(response)
        });
        if listing.data["pagination"]["stop_reason"] == "page_limit" {
            listing.error = None;
            listing.data["pagination"]["error"] = Value::Null;
        }
        if let Some(prior) = prior {
            merge_listing(&mut listing.data, &prior);
        }
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
    if listing.error.is_some() {
        return Err(
            "Inventory request failed; saved progress in listing.json. Resume later.".into(),
        );
    }
    let mut index = fs::File::create(output.join("index.jsonl.tmp"))?;
    let mut segment_index = fs::File::create(output.join("segments.jsonl.tmp"))?;
    let mut fetched = 0;
    let mut exported = 0;
    for speech in &speeches {
        let id = speech["otid"].as_str().ok_or("Missing OTID")?;
        // Encode every byte to avoid filesystem traversal and name collisions.
        let path = conversation_path(output, id);
        let relative = path.strip_prefix(output)?.to_string_lossy().into_owned();
        if unavailable.get(id).is_some() && !path.exists() {
            continue;
        }
        if !path.exists() && fetched >= max_new {
            continue;
        }
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            if path.exists() {
                let doc = read_document(&path)?;
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
            fetched += 1;
            if response.status == 404 {
                unavailable[id] = json!({"otid": id, "title": speech["title"], "http_status": 404,
                    "reason": "Otter did not return this conversation; not exported",
                    "observed_at_unix_seconds": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs()});
                save(&unavailable_path, &unavailable)?;
                return Ok(Value::Null);
            }
            check_response(&response, "speech", output)?;
            let doc = document(response.data);
            save_document(&path, &doc)?;
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
        if doc.is_null() {
            eprintln!("Recorded unavailable conversation (HTTP 404)");
            continue;
        }
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
            "Exported {}/{} ({fetched} detail requests this run)",
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
    let unavailable_count = speeches
        .iter()
        .filter(|speech| {
            speech["otid"]
                .as_str()
                .is_some_and(|id| unavailable.get(id).is_some())
        })
        .count();
    manifest["unavailable"] = json!(unavailable_count);
    manifest["finished"] =
        json!(listing.complete && exported + unavailable_count == speeches.len());
    manifest["complete"] = json!(listing.complete && exported == speeches.len());
    save(&output.join("manifest.json"), &manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_documents_round_trip_and_keep_legacy_files_readable() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("conversations")).unwrap();
        let doc = json!({"otid": "a", "text": "Unicode café", "segments": [{"start_ms": 12}]});
        let path = conversation_path(directory.path(), "a");
        assert_eq!(path.extension().unwrap(), "gz");
        save_document(&path, &doc).unwrap();
        assert_eq!(read_document(&path).unwrap(), doc);
        assert!(!path.with_extension("gz.tmp").exists());
        let legacy = directory.path().join("conversations/62.json");
        save(&legacy, &doc).unwrap();
        assert_eq!(conversation_path(directory.path(), "b"), legacy);
        assert_eq!(read_document(&legacy).unwrap(), doc);
    }

    #[test]
    fn resumed_listing_deduplicates_and_keeps_last_successful_cursor() {
        let prior = json!({"speeches": [{"otid": "a"}], "last_load_ts": 100});
        let mut next = json!({"speeches": [{"otid": "a"}, {"otid": "b"}], "last_load_ts": 50, "pagination": {"complete": false}});
        merge_listing(&mut next, &prior);
        assert_eq!(next["speeches"].as_array().unwrap().len(), 2);
        assert_eq!(next["last_load_ts"], 50);
        let mut failed = json!({"speeches": [], "pagination": {"complete": false}});
        merge_listing(&mut failed, &next);
        assert_eq!(failed["last_load_ts"], 50);
        assert_eq!(failed["speeches"].as_array().unwrap().len(), 2);
        assert_eq!(failed["pagination"]["complete"], false);
    }

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
