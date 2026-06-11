use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::auth::{authenticated_client, prompt};
use crate::util::{
    api, die, fail, format_duration, format_timestamp, print_json, resolve_folder_id, result_repr,
    truthy, value_str,
};

#[allow(clippy::too_many_arguments)]
pub fn list(folder: String, page_size: u32, source: String, days: Option<i64>, as_json: bool) {
    let client = authenticated_client();

    let folder_id = if !folder.is_empty() && folder.chars().all(|c| c.is_ascii_digit()) {
        folder
    } else {
        match resolve_folder_id(&client, &folder) {
            Ok(id) => id,
            Err(message) => die(message),
        }
    };

    let result = api(client.get_speeches(&folder_id, page_size, &source));
    if !result.ok() {
        fail(format!("Failed to get speeches: {}", result_repr(&result)));
    }

    let mut data = result.data;
    if let Some(days) = days {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs_f64();
        let cutoff = now - (days as f64) * 86400.0;
        if let Some(speeches) = data["speeches"].as_array() {
            let filtered: Vec<Value> = speeches
                .iter()
                .filter(|s| s["created_at"].as_f64().unwrap_or(0.0) >= cutoff)
                .cloned()
                .collect();
            data["speeches"] = Value::Array(filtered);
        }
    }

    if as_json {
        print_json(&data);
        return;
    }

    let speeches = data["speeches"].as_array().cloned().unwrap_or_default();
    if speeches.is_empty() {
        println!("No speeches found.");
        return;
    }

    println!("Found {} speeches:\n", speeches.len());
    for speech in &speeches {
        let title = match value_str(&speech["title"]) {
            t if t.is_empty() => "Untitled".to_string(),
            t => t,
        };
        let otid = value_str(&speech["otid"]);
        let live_tag = if value_str(&speech["live_status"]) == "live" {
            " [LIVE]"
        } else {
            ""
        };
        println!("  {otid}  {title}{live_tag}");

        let mut parts: Vec<String> = Vec::new();
        if truthy(&speech["created_at"]) {
            parts.push(format_timestamp(&speech["created_at"]));
        }
        if truthy(&speech["duration"]) {
            parts.push(format_duration(&speech["duration"]));
        }
        let folder_name = value_str(&speech["folder"]["folder_name"]);
        if !folder_name.is_empty() {
            parts.push(format!("📁 {folder_name}"));
        }
        let speakers = speaker_names(&speech["speakers"]);
        if !speakers.is_empty() {
            parts.push(format!("👤 {}", speakers.join(", ")));
        }
        if !parts.is_empty() {
            println!("           {}", parts.join(" | "));
        }
        println!();
    }
}

pub fn get(speech_id: String, as_json: bool) {
    let client = authenticated_client();
    let result = api(client.get_speech(&speech_id));
    if !result.ok() {
        fail(format!("Failed to get speech: {}", result_repr(&result)));
    }

    if as_json {
        print_json(&result.data);
        return;
    }

    let data = &result.data;
    let speech = &data["speech"];
    let title = match value_str(&speech["title"]) {
        t if t.is_empty() => "Untitled".to_string(),
        t => t,
    };
    println!("Title: {title}");
    println!("ID (otid): {}", value_str(&speech["otid"]));
    let created = &speech["created_at"];
    println!(
        "Created: {} ({})",
        format_timestamp(created),
        if created.is_null() {
            "0".into()
        } else {
            value_str(created)
        }
    );
    let duration = &speech["duration"];
    println!(
        "Duration: {} ({}s)",
        format_duration(duration),
        if duration.is_null() {
            "0".into()
        } else {
            value_str(duration)
        }
    );
    let folder = &speech["folder"];
    if folder.is_object() && truthy(&folder["folder_name"]) {
        println!(
            "Folder: {} (ID: {})",
            value_str(&folder["folder_name"]),
            value_str(&folder["id"])
        );
    }
    let speakers = speaker_names(&speech["speakers"]);
    if !speakers.is_empty() {
        println!("Speakers: {}", speakers.join(", "));
    }

    // Support both nested and top-level transcript formats.
    let transcripts = if truthy(&speech["transcripts"]) {
        &speech["transcripts"]
    } else {
        &data["transcripts"]
    };
    if let Some(segments) = transcripts.as_array() {
        if !segments.is_empty() {
            println!("\nTranscript:");
            println!("{}", "-".repeat(40));
            for segment in segments {
                let speaker = match value_str(&segment["speaker_name"]) {
                    s if s.is_empty() => "Unknown".to_string(),
                    s => s,
                };
                println!("[{speaker}]: {}", value_str(&segment["transcript"]));
            }
        }
    }
}

pub fn search(query: String, speech_id: String, size: u32, as_json: bool) {
    let client = authenticated_client();
    let result = api(client.query_speech(&query, &speech_id, size));
    if !result.ok() {
        fail(format!("Search failed: {}", result_repr(&result)));
    }

    let data = &result.data;
    if as_json {
        print_json(data);
        return;
    }

    // Python: matches = data.get("results") or data.get("matches") or data.get("items")
    let matches = if truthy(&data["results"]) {
        &data["results"]
    } else if truthy(&data["matches"]) {
        &data["matches"]
    } else {
        &data["items"]
    };

    match matches.as_array() {
        Some(items) if !items.is_empty() => {
            for (idx, item) in items.iter().enumerate() {
                let text = first_truthy(item, &["transcript", "text"]);
                let start = first_truthy(item, &["start_time", "start"]);
                let end = first_truthy(item, &["end_time", "end"]);
                let mut header = format!("[{}]", idx + 1);
                if !start.is_empty() || !end.is_empty() {
                    let range = format!("{start}-{end}");
                    header.push(' ');
                    header.push_str(range.trim_matches('-'));
                }
                println!("{header}");
                if !text.is_empty() {
                    println!("{text}");
                }
                println!();
            }
        }
        Some(_) => println!("No results found."),
        None => print_json(data),
    }
}

pub fn rename(speech_id: String, title: String) {
    let client = authenticated_client();
    let result = api(client.set_speech_title(&speech_id, &title));
    if !result.ok() {
        fail(format!("Rename failed: {}", result_repr(&result)));
    }
    println!("Renamed speech {speech_id} to: {title}");
}

pub fn download(speech_id: String, format: String, output: Option<String>) {
    let client = authenticated_client();
    let result = api(client.download_speech(&speech_id, output.as_deref(), &format));
    if !result.ok() {
        fail(format!("Download failed: {}", result_repr(&result)));
    }
    println!("Downloaded: {}", value_str(&result.data["filename"]));
}

pub fn upload(file: String, content_type: String) {
    if !std::path::Path::new(&file).exists() {
        die(format!(
            "Invalid value for 'FILE': Path '{file}' does not exist."
        ));
    }
    let client = authenticated_client();

    println!("Uploading {file}...");
    let result = api(client.upload_speech(&file, &content_type));
    if !result.ok() {
        fail(format!("Upload failed: {}", result_repr(&result)));
    }
    println!("Upload successful! Transcription is processing.");
    print_json(&result.data);
}

pub fn trash(speech_id: String, yes: bool) {
    if !yes {
        let answer = prompt(&format!("Move speech {speech_id} to trash? [y/N]: "));
        if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
            fail("Aborted!");
        }
    }

    let client = authenticated_client();
    let result = api(client.move_to_trash_bin(&speech_id));
    if !result.ok() {
        fail(format!("Failed to trash speech: {}", result_repr(&result)));
    }
    println!("Speech {speech_id} moved to trash.");
}

pub fn move_to_folder(speech_ids: Vec<String>, folder: String, create: bool) {
    let client = authenticated_client();

    let folder_id = if !folder.is_empty() && folder.chars().all(|c| c.is_ascii_digit()) {
        folder.clone()
    } else {
        match resolve_folder_id(&client, &folder) {
            Ok(id) => id,
            Err(message) if create => {
                let _ = message;
                let result = api(client.create_folder(&folder));
                if !result.ok() {
                    fail(format!("Failed to create folder: {}", result_repr(&result)));
                }
                let id = value_str(&result.data["folder"]["id"]);
                let id = if id.is_empty() { "unknown".into() } else { id };
                println!("Created folder '{folder}' (ID: {id})");
                id
            }
            Err(message) => die(message),
        }
    };

    let result = api(client.add_folder_speeches(&folder_id, &speech_ids));
    if !result.ok() {
        fail(format!("Failed to move speeches: {}", result_repr(&result)));
    }

    if speech_ids.len() == 1 {
        println!("Moved speech {} to folder {folder}", speech_ids[0]);
    } else {
        println!("Moved {} speeches to folder {folder}", speech_ids.len());
    }
}

fn speaker_names(speakers: &Value) -> Vec<String> {
    speakers
        .as_array()
        .map(|list| {
            list.iter()
                .map(|s| value_str(&s["speaker_name"]))
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn first_truthy(item: &Value, keys: &[&str]) -> String {
    for key in keys {
        if truthy(&item[*key]) {
            return value_str(&item[*key]);
        }
    }
    String::new()
}
