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
    transcript_uuid: Option<String>,
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

    if transcript_uuid.is_none() && !tag_all {
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
            println!("Use -t <uuid> to tag a specific segment, or --all to tag all.");
        }
        return;
    }

    let segments_to_tag: Vec<String> = if let Some(uuid) = transcript_uuid {
        vec![uuid]
    } else {
        transcripts
            .iter()
            .map(|t| value_str(&t["uuid"]))
            .filter(|uuid| !uuid.is_empty())
            .collect()
    };

    let mut tagged_count = 0;
    for uuid in &segments_to_tag {
        match client.set_transcript_speaker(&speech_id, uuid, &speaker_id, &speaker_name, false) {
            Ok(result) if result.ok() => {
                tagged_count += 1;
                if !tag_all {
                    println!("Tagged segment {uuid} as '{speaker_name}'");
                }
            }
            Ok(result) => eprintln!("Failed to tag {uuid}: {}", result_repr(&result)),
            Err(err) => eprintln!("Error tagging {uuid}: {err}"),
        }
    }

    if tag_all {
        println!(
            "Tagged {tagged_count}/{} segments as '{speaker_name}'",
            segments_to_tag.len()
        );
    }
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
