use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::authenticated_client_with_login;
use crate::util::{fail, print_json, value_str};

pub fn ask(question: String, as_json: bool, timeout_secs: u64, debug: bool) {
    let (client, login) = authenticated_client_with_login();

    // Token discovery: env var first, then scan login, then scan user.
    let mut debug_notes: Vec<String> = Vec::new();
    let token_source: String;
    let token_env = std::env::var("OTTERAI_WS_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_hit = token_env
        .as_ref()
        .map(|s| looks_like_jwt(s))
        .unwrap_or(false);
    let token = if let Some(env_tok) = token_env {
        token_source = "env:OTTERAI_WS_TOKEN".into();
        env_tok
    } else if let Some((path, tok)) = find_jwt_in_value(&login.data, "") {
        token_source = format!("login:{path}");
        tok
    } else {
        // Fallback: GET /user and scan its JSON
        let user = crate::util::api(client.get_user());
        if let Some((path, tok)) = find_jwt_in_value(&user.data, "") {
            token_source = format!("user:{path}");
            tok
        } else {
            fail("Could not locate a websocket token for AI Chat. Run with --debug for hints.");
        }
    };
    if debug {
        // Collect key names for login and user
        let login_keys = login
            .data
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        debug_notes.push(format!("login keys: {}", login_keys.join(", ")));
        let user = crate::util::api(client.get_user());
        let user_keys = user
            .data
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        debug_notes.push(format!("user keys: {}", user_keys.join(", ")));

        // Scan hits by location (without printing values)
        eprintln!("Debug: using websocket token from {}", token_source);
        eprintln!("Debug: env: {}", if env_hit { "hit" } else { "no" });
        if let Some((path, _)) = find_jwt_in_value(&login.data, "") {
            eprintln!("Debug: login.json: hit at {path}");
        } else {
            eprintln!("Debug: login.json: no");
        }
        if let Some((path, _)) = find_jwt_in_value(&user.data, "") {
            eprintln!("Debug: user.json: hit at {path}");
        } else {
            eprintln!("Debug: user.json: no");
        }
        let header_hits = client
            .debug_login_header_jwt_scan()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, hit)| if hit { Some(name) } else { None })
            .collect::<Vec<_>>();
        eprintln!(
            "Debug: login.headers: {}",
            if header_hits.is_empty() {
                "no".into()
            } else {
                format!("hit at [{}]", header_hits.join(", "))
            }
        );
        let cookies = client.debug_cookie_names_and_jwt_hits();
        let cookie_report = if cookies.is_empty() {
            "none".to_string()
        } else {
            cookies
                .into_iter()
                .map(|(name, hit)| format!("{name}:{}", if hit { "hit" } else { "no" }))
                .collect::<Vec<_>>()
                .join(", ")
        };
        eprintln!("Debug: cookies: {cookie_report}");
        for note in debug_notes {
            eprintln!("Debug: {note}");
        }
    }

    // Open websocket BEFORE sending the chat message to avoid missing frames.
    let thread_uuid = Uuid::new_v4().to_string();
    let ws_url = format!(
        "wss://ws.aisense.com/api/v2/client/session_update?token={}",
        urlencoding::encode(&token)
    );
    let request = {
        use tungstenite::client::IntoClientRequest;
        let mut req = ws_url
            .as_str()
            .into_client_request()
            .expect("ws url parses into request");
        // Match the browser's Origin to avoid cross-origin rejections.
        req.headers_mut()
            .insert("Origin", "https://otter.ai".parse().unwrap());
        req
    };
    let (mut socket, _response) = match tungstenite::client::connect(request) {
        Ok(ok) => ok,
        Err(err) => fail(format!("Failed to open websocket: {err}")),
    };

    // Send the question.
    let ack = crate::util::api(client.send_chat_message(&thread_uuid, &question));
    if !ack.ok() {
        fail(format!(
            "Failed to send chat message: {}",
            crate::util::result_repr(&ack)
        ));
    }
    let message_uuid = value_str(&ack.data["message_uuid"]);
    if message_uuid.is_empty() {
        fail("Chat acknowledgement did not include message_uuid; cannot receive answer.");
    }
    let session_uuid = value_str(&ack.data["session_uuid"]);
    let effective_thread_uuid = if session_uuid.is_empty() {
        thread_uuid.clone()
    } else {
        session_uuid
    };

    // Read updates until finished for this message_uuid, with timeout.
    let (tx, rx) = mpsc::channel::<Value>();
    let target_uuid = message_uuid.clone();
    std::thread::spawn(move || {
        let mut latest: Option<Value> = None;
        loop {
            match socket.read() {
                Ok(msg) if msg.is_text() => {
                    if let Ok(v) =
                        serde_json::from_str::<Value>(&msg.into_text().unwrap_or_default())
                    {
                        if v["status"]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case("OK"))
                            && v["type"] == "chat_message"
                            && v["action"] == "update"
                        {
                            let message = &v["message"];
                            if value_str(&message["chat_message_uuid"]) == target_uuid {
                                latest = Some(message.clone());
                                if message["finished"].as_bool() == Some(true) {
                                    let _ = tx.send(message.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                Ok(_) => {}      // ignore non-text frames
                Err(_) => break, // socket closed or error
            }
        }
        if let Some(v) = latest {
            let _ = tx.send(v);
        }
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let remaining = deadline.saturating_duration_since(Instant::now());
    let message = match rx.recv_timeout(remaining) {
        Ok(v) => v,
        Err(_) => fail(format!(
            "Timed out waiting for AI Chat answer after {} seconds",
            timeout_secs
        )),
    };
    let blocks = message["blocks"].clone();
    let answer_text = render_blocks(&blocks);
    let sources = extract_sources(&blocks);

    if as_json {
        let out = json!({
            "question": question,
            "answer_text": answer_text,
            "blocks": blocks,
            "sources": sources,
            "thread_uuid": effective_thread_uuid,
            "message_uuid": message_uuid
        });
        print_json(&out);
    } else {
        if answer_text.trim().is_empty() {
            println!("[No answer text returned]");
        } else {
            println!("{answer_text}");
        }
        if !sources.is_empty() {
            println!("\nSources:");
            for s in sources {
                println!("- {s}");
            }
        }
    }
}

fn find_jwt_in_value(root: &Value, path: &str) -> Option<(String, String)> {
    match root {
        Value::String(s) if looks_like_jwt(s) => Some((path.into(), s.clone())),
        Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if let Some(found) = find_jwt_in_value(v, &child) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for (idx, v) in items.iter().enumerate() {
                let child = format!("{path}[{idx}]");
                if let Some(found) = find_jwt_in_value(v, &child) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn looks_like_jwt(s: &str) -> bool {
    let parts = s.split('.').take(3).collect::<Vec<_>>();
    if parts.len() != 3 {
        return false;
    }
    // Basic quick checks without decoding: header starts with eyJ (base64url of {"..."}),
    // and each part has only URL-safe base64 characters.
    if !parts[0].starts_with("eyJ") {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

fn render_blocks(blocks: &Value) -> String {
    let mut out = String::new();
    let mut wrote_something = false;
    if let Some(items) = blocks.as_array() {
        for block in items {
            let btype = block["type"].as_str().unwrap_or_default();
            let children = block["children"].as_array().cloned().unwrap_or_default();
            if btype == "list" {
                for child in children {
                    let text = value_str(&child["text"]).trim().to_string();
                    if !text.is_empty() {
                        out.push_str("- ");
                        out.push_str(&text);
                        out.push('\n');
                        wrote_something = true;
                    }
                }
            } else {
                let mut para = String::new();
                for child in children {
                    let text = value_str(&child["text"]);
                    if !text.is_empty() {
                        if !para.is_empty() {
                            para.push(' ');
                        }
                        para.push_str(&text);
                    }
                }
                if !para.is_empty() {
                    if wrote_something && !out.ends_with("\n\n") {
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push('\n');
                    }
                    out.push_str(&para);
                    out.push('\n');
                    wrote_something = true;
                }
            }
        }
    }
    out.trim_end().to_string()
}

fn extract_sources(blocks: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    fn scrape(value: &Value, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>) {
        match value {
            Value::Object(map) => {
                if let Some(url) = map.get("url").and_then(Value::as_str) {
                    if let Some(otid) = otid_from_url(url) {
                        if seen.insert(otid.clone()) {
                            out.push(otid);
                        }
                    }
                }
                for v in map.values() {
                    scrape(v, out, seen);
                }
            }
            Value::Array(items) => {
                for v in items {
                    scrape(v, out, seen);
                }
            }
            _ => {}
        }
    }
    scrape(blocks, &mut out, &mut seen);
    out
}

fn otid_from_url(url: &str) -> Option<String> {
    // Accept absolute or relative otter links: https://otter.ai/u/<otid>?..., or /u/<otid>
    let needle = "/u/";
    let locate = url.find(needle)?;
    let rest = &url[locate + needle.len()..];
    let end = rest
        .find(|c: char| c == '?' || c == '#' || c == '/' || c.is_whitespace())
        .unwrap_or(rest.len());
    let otid = &rest[..end];
    if otid.is_empty() {
        None
    } else {
        Some(otid.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn looks_like_jwt_detects_basic_shape() {
        assert!(looks_like_jwt(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.aaa.bbb"
        ));
        assert!(!looks_like_jwt("not-a-jwt"));
        assert!(!looks_like_jwt("eyJ.invalid"));
        assert!(!looks_like_jwt("eyJ$.bad.chars"));
    }

    #[test]
    fn render_blocks_supports_text_and_list_items() {
        let blocks = json!([
            {"type":"text","children":[{"text":"Hello world"},{"text":"from Otter"}]},
            {"type":"list","children":[{"text":"One"},{"text":"Two"}]}
        ]);
        let text = render_blocks(&blocks);
        assert!(text.contains("Hello world from Otter"));
        assert!(text.contains("- One"));
        assert!(text.contains("- Two"));
    }

    #[test]
    fn extract_sources_finds_otids_in_links() {
        let blocks = json!([
            {"type":"text","children":[
                {"text":"See "},
                {"url":"https://otter.ai/u/abc123?viaMessage=true","text":"source"}
            ]},
            {"type":"list","children":[
                {"text":"- item with /u/def456"}
            ]}
        ]);
        let sources = extract_sources(&blocks);
        assert!(sources.contains(&"abc123".to_string()));
        // The second isn't a link; only URL fields are parsed.
        assert!(!sources.contains(&"def456".to_string()));
    }
}
