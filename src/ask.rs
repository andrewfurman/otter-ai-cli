use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::authenticated_client_with_login;
use crate::util::{fail, print_json, value_str};

pub fn ask(question: String, as_json: bool, timeout_secs: u64, debug: bool) {
    let (client, login) = authenticated_client_with_login();

    // Debug pre-scan: headers, cookies, login/user/speeches JSON JWTs.
    let _debug_notes: Vec<String> = Vec::new();
    let header_hits = client
        .debug_login_header_jwt_scan()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, hit)| if hit { Some(name) } else { None })
        .collect::<Vec<_>>();
    let cookie_hits = client.debug_cookie_names_and_jwt_hits();
    let user = crate::util::api(client.get_user());
    let login_jwts = collect_jwts(&login.data, "");
    let user_jwts = collect_jwts(&user.data, "");
    // Optional: scan one speeches page for debug only (not used as a token)
    let speeches_jwts = {
        let resp = crate::util::api(client.get_speeches("0", 1, "owned"));
        collect_jwts(&resp.data, "")
    };

    // Token discovery: env var first; then /forward/api/v1/get_jwt_token (GET first, then POST {})
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
    } else {
        // Always print the debug report before attempting get_jwt_token.
        if debug {
            eprintln!("Debug: env: {}", if env_hit { "hit" } else { "no" });
            eprintln!(
                "Debug: login.headers: {}",
                if header_hits.is_empty() {
                    "no".into()
                } else {
                    format!("hit at [{}]", header_hits.join(", "))
                }
            );
            let cookie_report = if cookie_hits.is_empty() {
                "none".to_string()
            } else {
                cookie_hits
                    .iter()
                    .map(|(name, hit)| format!("{name}:{}", if *hit { "hit" } else { "no" }))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            eprintln!("Debug: cookies: {cookie_report}");
            print_jwt_report("login.json", &login_jwts);
            print_jwt_report("user.json", &user_jwts);
            print_jwt_report("speeches.json", &speeches_jwts);
        }
        // Try GET then POST to /get_jwt_token
        let get = crate::util::api(client.get_ws_token_get());
        if debug {
            eprintln!("Debug: get_jwt_token GET status={}", get.status);
        }
        if get.ok() {
            if let Some(tok) = get.data.get("token").and_then(Value::as_str) {
                token_source = "get_jwt_token:GET".into();
                tok.to_string()
            } else {
                // Fallback to POST
                let post = crate::util::api(client.get_ws_token_post());
                if debug {
                    eprintln!("Debug: get_jwt_token POST status={}", post.status);
                }
                if post.ok() {
                    if let Some(tok) = post.data.get("token").and_then(Value::as_str) {
                        token_source = "get_jwt_token:POST".into();
                        tok.to_string()
                    } else {
                        fail("Could not locate a websocket token for AI Chat. Run with --debug for hints.");
                    }
                } else {
                    fail("Could not locate a websocket token for AI Chat. Run with --debug for hints.");
                }
            }
        } else {
            // Try POST when GET not OK
            let post = crate::util::api(client.get_ws_token_post());
            if debug {
                eprintln!("Debug: get_jwt_token POST status={}", post.status);
            }
            if post.ok() {
                if let Some(tok) = post.data.get("token").and_then(Value::as_str) {
                    token_source = "get_jwt_token:POST".into();
                    tok.to_string()
                } else {
                    fail("Could not locate a websocket token for AI Chat. Run with --debug for hints.");
                }
            } else {
                fail("Could not locate a websocket token for AI Chat. Run with --debug for hints.");
            }
        }
    };
    if debug {
        eprintln!("Debug: using websocket token from {}", token_source);
    }

    // Open websocket BEFORE sending the chat message to avoid missing frames.
    let thread_uuid = Uuid::new_v4().to_string();
    // Extract numeric user id from login response for WS URL
    let user_id = value_str(&login.data["userid"]);
    if user_id.is_empty() {
        let msg = "Login data did not include a valid userid".to_string();
        if as_json {
            let obj = json!({"error": msg});
            print_json(&obj);
            std::process::exit(1);
        } else {
            fail(msg);
        }
    }
    let dbg_url = format!(
        "wss://ws.aisense.com/api/v2/client/session_update?token={}&userid={}",
        "<REDACTED>", user_id
    );
    let connect_started = Instant::now();
    let (mut socket, status_code) = match connect_ws(&token, &user_id) {
        Ok(ok) => ok,
        Err(_err) => fail("Failed to open websocket with the selected token"),
    };
    if debug {
        eprintln!("Debug: ws url={} (masked)", dbg_url);
        eprintln!("Debug: handshake OK (status={})", status_code);
    }
    // Send an immediate app-level ping after connect.
    let _ = socket.write(tungstenite::Message::Text(r#"{"action":"ping"}"#.into()));

    // Send the question.
    let ack = crate::util::api(client.send_chat_message(&thread_uuid, &question));
    if !ack.ok() {
        let msg = format!(
            "Failed to send chat message: {}",
            crate::util::result_repr(&ack)
        );
        if as_json {
            let obj = json!({"error": msg});
            print_json(&obj);
            std::process::exit(1);
        } else {
            fail(msg);
        }
    }
    let message_obj = &ack.data["message"];
    let message_uuid = value_str(&message_obj["message_uuid"]);
    if message_uuid.is_empty() {
        if debug {
            eprintln!("Debug: POST ack keys: {}", list_paths(&ack.data).join(", "));
        }
        let msg =
            "Chat acknowledgement did not include message_uuid; cannot receive answer.".to_string();
        if as_json {
            let obj = json!({"error": msg});
            print_json(&obj);
            std::process::exit(1);
        } else {
            fail(msg);
        }
    }
    let session_uuid = value_str(&message_obj["session_uuid"]);
    let effective_thread_uuid = if session_uuid.is_empty() {
        thread_uuid.clone()
    } else {
        session_uuid
    };

    // Read updates until finished for this message_uuid, with timeout.
    struct ChatUpdate {
        message: Value,
        received: usize,
        matched: usize,
        closed: Option<String>,
        error: Option<String>,
        elapsed_ms: u128,
    }
    let (tx, rx) = mpsc::channel::<ChatUpdate>();
    let target_uuid = message_uuid.clone();
    let thread_uuid_clone = thread_uuid.clone();
    std::thread::spawn(move || {
        let mut received = 0usize;
        let mut matched = 0usize;
        let mut last_ping = Instant::now();
        loop {
            match socket.read() {
                Ok(msg) if msg.is_text() => {
                    received += 1;
                    if let Ok(v) =
                        serde_json::from_str::<Value>(&msg.into_text().unwrap_or_default())
                    {
                        // Per-frame debug summary
                        if debug {
                            let top_keys = v
                                .as_object()
                                .map(|m| m.keys().cloned().collect::<Vec<_>>())
                                .unwrap_or_default();
                            let mkeys = v
                                .get("message")
                                .and_then(|m| m.as_object())
                                .map(|m| m.keys().cloned().collect::<Vec<_>>())
                                .unwrap_or_default();
                            let t = v.get("type").and_then(Value::as_str).unwrap_or_default();
                            let a = v.get("action").and_then(Value::as_str).unwrap_or_default();
                            let message = &v["message"];
                            let tmatch = value_str(&message["thread_uuid"]) == thread_uuid_clone;
                            let echo_uuid = value_str(&message["uuid"]) == target_uuid;
                            let author = value_str(&message["author"]).to_ascii_lowercase();
                            let is_user = author == "user";
                            eprintln!(
                                "Debug: frame type={} action={} top=[{}] message_keys=[{}] tmatch={} echo_uuid={} is_user={}",
                                t,
                                a,
                                top_keys.join(","),
                                mkeys.join(","),
                                tmatch,
                                echo_uuid,
                                is_user
                            );
                        }
                        if v["status"]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case("OK"))
                            && v["type"] == "chat_message"
                            && v["action"] == "update"
                        {
                            let message = &v["message"];
                            let tmatch = value_str(&message["thread_uuid"]) == thread_uuid_clone;
                            let echo_uuid = value_str(&message["uuid"]) == target_uuid;
                            let author = value_str(&message["author"]).to_ascii_lowercase();
                            let is_user = author == "user";
                            if tmatch && !(echo_uuid || is_user) {
                                matched += 1;
                                if message["finished"].as_bool() == Some(true) {
                                    let _ = tx.send(ChatUpdate {
                                        message: message.clone(),
                                        received,
                                        matched,
                                        closed: None,
                                        error: None,
                                        elapsed_ms: connect_started.elapsed().as_millis(),
                                    });
                                    return;
                                }
                            }
                        }
                        if last_ping.elapsed() >= Duration::from_secs(15) {
                            let _ = socket
                                .write(tungstenite::Message::Text(r#"{"action":"ping"}"#.into()));
                            last_ping = Instant::now();
                        }
                    }
                }
                Ok(msg) if msg.is_ping() => {
                    let payload = msg.into_data();
                    let _ = socket.write(tungstenite::Message::Pong(payload));
                    continue;
                }
                Ok(msg) if msg.is_close() => {
                    let reason = if let tungstenite::Message::Close(Some(cf)) = msg {
                        format!("code={} reason={}", cf.code, cf.reason)
                    } else {
                        "unknown".into()
                    };
                    let _ = tx.send(ChatUpdate {
                        message: Value::Null,
                        received,
                        matched,
                        closed: Some(reason),
                        error: None,
                        elapsed_ms: connect_started.elapsed().as_millis(),
                    });
                    return;
                }
                Ok(_) => {} // ignore non-text frames
                Err(err) => {
                    if let tungstenite::Error::Io(ioe) = &err {
                        if ioe.kind() == std::io::ErrorKind::WouldBlock
                            || ioe.kind() == std::io::ErrorKind::TimedOut
                        {
                            if last_ping.elapsed() >= Duration::from_secs(15) {
                                let _ = socket.write(tungstenite::Message::Text(
                                    r#"{"action":"ping"}"#.into(),
                                ));
                                last_ping = Instant::now();
                            }
                            continue;
                        }
                    }
                    match err {
                        tungstenite::Error::ConnectionClosed
                        | tungstenite::Error::AlreadyClosed => {
                            let _ = tx.send(ChatUpdate {
                                message: Value::Null,
                                received,
                                matched,
                                closed: Some("closed".into()),
                                error: None,
                                elapsed_ms: connect_started.elapsed().as_millis(),
                            });
                        }
                        other => {
                            let _ = tx.send(ChatUpdate {
                                message: Value::Null,
                                received,
                                matched,
                                closed: None,
                                error: Some(format!("{other}")),
                                elapsed_ms: connect_started.elapsed().as_millis(),
                            });
                        }
                    }
                    return;
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let remaining = deadline.saturating_duration_since(Instant::now());
    let update = match rx.recv_timeout(remaining) {
        Ok(v) => v,
        Err(_) => {
            let msg = format!(
                "Timed out waiting for AI Chat answer after {} seconds",
                timeout_secs
            );
            if as_json {
                let obj = json!({"error": msg});
                print_json(&obj);
                std::process::exit(1);
            } else {
                fail(msg);
            }
        }
    };
    if debug {
        if let Some(reason) = &update.closed {
            eprintln!(
                "Debug: websocket closed: {} (frames received={} matched={} elapsed={}ms)",
                reason, update.received, update.matched, update.elapsed_ms
            );
        } else if let Some(err) = &update.error {
            eprintln!(
                "Debug: websocket error: {} (frames received={} matched={} elapsed={}ms)",
                err, update.received, update.matched, update.elapsed_ms
            );
        } else {
            eprintln!(
                "Debug: frames received={} matched={} elapsed={}ms",
                update.received, update.matched, update.elapsed_ms
            );
        }
    }
    let message = update.message;
    let blocks = message["blocks"].clone();
    let mut sources_list: Vec<SourceInfo> = Vec::new();
    let mut answer_text = render_blocks(&blocks, &mut sources_list);
    if answer_text.trim().is_empty() {
        let fallback = value_str(&message["text"]);
        if !fallback.trim().is_empty() {
            answer_text = fallback;
        }
    }
    // Merge in any additional sources not seen during rendering (URL nodes etc.)
    collect_sources_from_urls(&blocks, &mut sources_list);

    if answer_text.trim().is_empty() {
        let errmsg = if let Some(reason) = update.closed {
            format!("websocket closed: {}", reason)
        } else if let Some(err) = update.error {
            format!("websocket error: {err}")
        } else {
            "no answer text returned".into()
        };
        if as_json {
            let error_obj = json!({
                "error": errmsg,
                "frames_received": update.received,
                "frames_matched": update.matched
            });
            print_json(&error_obj);
        } else {
            eprintln!("Error: {}", errmsg);
        }
        std::process::exit(1);
    }

    if as_json {
        let out = json!({
            "question": question,
            "answer_text": answer_text,
            "blocks": blocks,
            "sources": sources_list.iter().map(|s| json!({
                "title": s.title,
                "otid": s.otid,
                "speech_id": s.speech_id,
                "start_time": s.start_time
            })).collect::<Vec<_>>(),
            "thread_uuid": effective_thread_uuid,
            "message_uuid": message_uuid
        });
        print_json(&out);
    } else {
        println!("{answer_text}");
        if !sources_list.is_empty() {
            println!("\nSources:");
            for s in &sources_list {
                let title = if s.title.is_empty() {
                    "(untitled)"
                } else {
                    &s.title
                };
                println!("- {} [{}]", title, s.otid);
            }
        }
    }
}

fn normalize_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Clone)]
struct FoundJwt {
    path: String,
    token: String,
}

fn collect_jwts(root: &Value, path: &str) -> Vec<FoundJwt> {
    let mut out = Vec::new();
    fn walk(node: &Value, path: &str, out: &mut Vec<FoundJwt>) {
        match node {
            Value::String(s) if looks_like_jwt(s) => out.push(FoundJwt {
                path: path.into(),
                token: s.clone(),
            }),
            Value::Object(map) => {
                for (k, v) in map {
                    let child = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(v, &child, out);
                }
            }
            Value::Array(items) => {
                for (idx, v) in items.iter().enumerate() {
                    let child = format!("{path}[{idx}]");
                    walk(v, &child, out);
                }
            }
            _ => {}
        }
    }
    walk(root, path, &mut out);
    out
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

fn list_paths(root: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(node: &Value, path: &str, out: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                if !path.is_empty() {
                    out.push(path.into());
                }
                for (k, v) in map {
                    let child = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(v, &child, out);
                }
            }
            Value::Array(items) => {
                for (idx, v) in items.iter().enumerate() {
                    let child = format!("{path}[{idx}]");
                    walk(v, &child, out);
                }
            }
            _ => {
                if !path.is_empty() {
                    out.push(path.into());
                }
            }
        }
    }
    walk(root, "", &mut out);
    out
}

fn print_jwt_report(label: &str, items: &[FoundJwt]) {
    if items.is_empty() {
        eprintln!("Debug: {label}: no JWT-like strings");
        return;
    }
    for item in items {
        let (alg, claims) = jwt_metadata(&item.token);
        eprintln!(
            "Debug: {label}: path={} len={} alg={} claims=[{}]",
            item.path,
            item.token.len(),
            alg.unwrap_or_else(|| "unknown".into()),
            claims.join(", ")
        );
    }
}

fn jwt_metadata(token: &str) -> (Option<String>, Vec<String>) {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return (None, Vec::new());
    }
    let header = URL_SAFE_NO_PAD.decode(parts[0]).ok();
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).ok();
    let alg = header
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v.get("alg").and_then(Value::as_str).map(|s| s.to_string()));
    let claims = payload
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()))
        .unwrap_or_default();
    (alg, claims)
}

struct ConnectError;

fn connect_ws(
    token: &str,
    userid: &str,
) -> Result<
    (
        tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
        u16,
    ),
    ConnectError,
> {
    let ws_url = format!(
        "wss://ws.aisense.com/api/v2/client/session_update?token={}&userid={}",
        urlencoding::encode(token),
        urlencoding::encode(userid)
    );
    use tungstenite::client::IntoClientRequest;
    let mut req = ws_url
        .as_str()
        .into_client_request()
        .expect("ws url parses into request");
    req.headers_mut()
        .insert("Origin", "https://otter.ai".parse().unwrap());
    req.headers_mut().insert(
        "User-Agent",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) OtterCLI/0.1 Safari/537.36"
            .parse()
            .unwrap(),
    );
    match tungstenite::client::connect(req) {
        Ok((sock, resp)) => Ok((sock, resp.status().as_u16())),
        Err(_err) => Err(ConnectError),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourceInfo {
    title: String,
    otid: String,
    speech_id: String,
    start_time: String,
}

fn render_blocks(blocks: &Value, sources: &mut Vec<SourceInfo>) -> String {
    let mut out = String::new();
    render_nodes(blocks, 0, false, &mut out, sources);
    out.trim_end().to_string()
}

fn render_nodes(
    node: &Value,
    indent: usize,
    in_list_item: bool,
    out: &mut String,
    sources: &mut Vec<SourceInfo>,
) {
    match node {
        Value::Array(items) => {
            for item in items {
                render_nodes(item, indent, in_list_item, out, sources);
            }
        }
        Value::Object(map) => {
            let node_type = map.get("type").and_then(Value::as_str).unwrap_or("");
            match node_type {
                "list_block" => {
                    if let Some(children) = map.get("children") {
                        render_nodes(children, indent, false, out, sources);
                    }
                }
                "list_item" => {
                    // Otter's real shape: each list_item contains multiple text_blocks.
                    // - The first text_block is the bullet line content.
                    // - Subsequent text_blocks begin with a literal "  - " marker; render each
                    //   on its own indented sub-line with a normalized "- " prefix.
                    // - Inline nodes like `speech` appear as siblings; keep them with the current group.
                    if let Some(children) = map.get("children") {
                        // Group contiguous inline content by top-level text_block boundaries.
                        let mut groups: Vec<String> = Vec::new();
                        let mut current: String = String::new();
                        let push_current = |groups: &mut Vec<String>, current: &mut String| {
                            let text = normalize_inline(current);
                            if !text.is_empty() {
                                groups.push(text);
                            }
                            current.clear();
                        };

                        if let Some(items) = children.as_array() {
                            for item in items {
                                let item_type =
                                    item.get("type").and_then(Value::as_str).unwrap_or("");
                                if item_type == "text_block" {
                                    // Finish previous group.
                                    push_current(&mut groups, &mut current);
                                    // Collect this block's inline content.
                                    if let Some(inline) = item.get("children") {
                                        collect_inline(inline, &mut current, sources);
                                    }
                                    // Keep as current group (do not push yet) to allow following
                                    // inline siblings (e.g., `speech` nodes) to join this line.
                                    push_current(&mut groups, &mut current);
                                } else if item_type == "list_block" || item_type == "list_item" {
                                    // Rare nested list: finish current groups and render nested.
                                    push_current(&mut groups, &mut current);
                                    render_nodes(item, indent + 1, false, out, sources);
                                } else {
                                    // Inline sibling: append to current group.
                                    collect_inline(item, &mut current, sources);
                                }
                            }
                            // Push any trailing inline group
                            push_current(&mut groups, &mut current);
                        }

                        // If the entire list_item is only hyphens (e.g., "--"), render a divider.
                        let mut all_text = groups.join(" ");
                        all_text.retain(|c| !c.is_whitespace());
                        if !all_text.is_empty() && all_text.chars().all(|c| c == '-') {
                            if !out.ends_with('\n') {
                                out.push('\n');
                            }
                            out.push_str(&"  ".repeat(indent));
                            out.push_str("---\n");
                            return;
                        }

                        if !groups.is_empty() {
                            // Main bullet line
                            let first = groups[0].as_str();
                            if !first.is_empty() {
                                out.push_str(&"  ".repeat(indent));
                                out.push_str("- ");
                                out.push_str(first);
                                out.push('\n');
                            }
                            // Sub-lines: strip any leading literal "  - " marker and re-indent.
                            for sub in groups.iter().skip(1) {
                                let mut subline = strip_leading_marker(sub);
                                subline = normalize_inline(&subline);
                                if !subline.is_empty() {
                                    out.push_str(&"  ".repeat(indent + 1));
                                    out.push_str("- ");
                                    out.push_str(&subline);
                                    out.push('\n');
                                }
                            }
                        }
                    }
                }
                "divider" | "hr" => {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&"  ".repeat(indent));
                    out.push_str("---\n");
                }
                "text_block" => {
                    // Paragraph of inline text
                    let mut para = String::new();
                    if let Some(children) = map.get("children") {
                        collect_inline(children, &mut para, sources);
                    }
                    let trimmed = para.trim_end();
                    if !trimmed.is_empty() {
                        // Paragraph separation
                        if !out.is_empty() && !out.ends_with("\n\n") {
                            if !out.ends_with('\n') {
                                out.push('\n');
                            }
                            out.push('\n');
                        }
                        out.push_str(&normalize_inline(trimmed));
                        out.push('\n');
                    }
                }
                "text" => {
                    // Paragraph-like text node: include children as inline, then newline
                    let mut para = String::new();
                    if let Some(t) = map.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            para.push_str(t);
                        }
                    }
                    if let Some(children) = map.get("children") {
                        collect_inline(children, &mut para, sources);
                    }
                    let trimmed = para.trim_end();
                    if !trimmed.is_empty() {
                        if !out.is_empty() && !out.ends_with("\n\n") {
                            if !out.ends_with('\n') {
                                out.push('\n');
                            }
                            out.push('\n');
                        }
                        out.push_str(&normalize_inline(trimmed));
                        out.push('\n');
                    }
                }
                "speech" => {
                    // Inline citation: title text + [otid]
                    let mut title = map
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    title = normalize_title(&title);
                    let otid = value_str(map.get("otid").unwrap_or(&Value::Null));
                    if !title.is_empty() {
                        if in_list_item && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(&title);
                        if !otid.is_empty() {
                            out.push(' ');
                            out.push('[');
                            out.push_str(&otid);
                            out.push(']');
                        }
                    }
                    // Collect source
                    if !otid.is_empty() {
                        let speech_id = value_str(map.get("speech_id").unwrap_or(&Value::Null));
                        let start_time = value_str(map.get("start_time").unwrap_or(&Value::Null));
                        add_source(
                            sources,
                            SourceInfo {
                                title,
                                otid,
                                speech_id,
                                start_time,
                            },
                        );
                    }
                    // Recurse into any children of the speech node as well
                    if let Some(children) = map.get("children") {
                        render_nodes(children, indent, in_list_item, out, sources);
                    }
                }
                _ => {
                    // Unknown node: recurse into children/content to avoid dropping text
                    if let Some(children) = map.get("children") {
                        render_nodes(children, indent, in_list_item, out, sources);
                    }
                    if let Some(content) = map.get("content") {
                        render_nodes(content, indent, in_list_item, out, sources);
                    }
                }
            }
        }
        _ => {}
    }
}

// Normalize inline text: collapse runs of spaces, remove space before commas, trim end.
fn normalize_inline(s: &str) -> String {
    // Collapse multiple spaces into one (leave other whitespace alone).
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch == ' ' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    // Remove stray space before commas.
    let mut collapsed = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' && matches!(chars.peek(), Some(',')) {
            // skip this space
            continue;
        }
        collapsed.push(c);
    }
    collapsed.trim_end().to_string()
}

// Strip a leading literal list marker such as "- " or "  - " from a line.
fn strip_leading_marker(s: &str) -> String {
    let mut t = s.trim_start();
    if t.starts_with("- ") {
        t = &t[2..];
        t = t.trim_start();
    }
    t.to_string()
}

fn collect_inline(node: &Value, out: &mut String, sources: &mut Vec<SourceInfo>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_inline(item, out, sources);
            }
        }
        Value::Object(map) => {
            let node_type = map.get("type").and_then(Value::as_str).unwrap_or("");
            // Some nodes may be untyped text containers with a "text" field
            if node_type.is_empty() {
                if let Some(t) = map.get("text").and_then(Value::as_str) {
                    if !t.is_empty() {
                        if !out.is_empty() && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(t);
                        return;
                    }
                }
            }
            // Skip nested list blocks/items for inline collection; they render separately
            if node_type == "list_block" || node_type == "list_item" || node_type == "divider" {
                return;
            }
            match node_type {
                "text" => {
                    // Treat text nodes as inline content; if they have children, include them
                    let mut para = String::new();
                    if let Some(t) = map.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            para.push_str(t);
                        }
                    }
                    if let Some(children) = map.get("children") {
                        collect_inline(children, &mut para, sources);
                    }
                    if !para.is_empty() {
                        // If the last appended content ended with a citation "]", and this text
                        // node begins with a closing bracket, drop the extraneous bracket.
                        if out.ends_with(']') {
                            let trimmed = para.trim_start();
                            if trimmed.starts_with(']') {
                                // remove the leading bracket and any extra leading whitespace
                                let mut chars = trimmed.chars();
                                // skip the first ']' char
                                chars.next();
                                let remainder: String = chars.collect();
                                para = remainder.trim_start().to_string();
                            }
                        }
                        if !out.is_empty() && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(&para);
                    }
                }
                "text_block" => {
                    if let Some(children) = map.get("children") {
                        collect_inline(children, out, sources);
                    }
                }
                "speech" => {
                    // Drop any preceding " [" bracket from sibling text nodes.
                    strip_trailing_open_bracket(out);
                    let title = map
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let otid = value_str(map.get("otid").unwrap_or(&Value::Null));
                    if !title.is_empty() {
                        if !out.is_empty() && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push_str(&title);
                        if !otid.is_empty() {
                            out.push(' ');
                            out.push('[');
                            out.push_str(&otid);
                            out.push(']');
                        }
                    }
                    if !otid.is_empty() {
                        let speech_id = value_str(map.get("speech_id").unwrap_or(&Value::Null));
                        let start_time = value_str(map.get("start_time").unwrap_or(&Value::Null));
                        add_source(
                            sources,
                            SourceInfo {
                                title,
                                otid,
                                speech_id,
                                start_time,
                            },
                        );
                    }
                }
                _ => {
                    if let Some(children) = map.get("children") {
                        collect_inline(children, out, sources);
                    }
                    if let Some(content) = map.get("content") {
                        collect_inline(content, out, sources);
                    }
                }
            }
        }
        _ => {}
    }
}

fn strip_trailing_open_bracket(out: &mut String) {
    // Remove any trailing spaces, then a single '[' (and surrounding spaces) if present.
    while out.ends_with(' ') {
        out.pop();
    }
    if out.ends_with('[') {
        out.pop();
        while out.ends_with(' ') {
            out.pop();
        }
    }
}

fn add_source(sources: &mut Vec<SourceInfo>, src: SourceInfo) {
    if !sources.iter().any(|s| s.otid == src.otid) {
        sources.push(src);
    }
}

fn collect_sources_from_urls(node: &Value, sources: &mut Vec<SourceInfo>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_sources_from_urls(item, sources);
            }
        }
        Value::Object(map) => {
            if let Some(url) = map.get("url").and_then(Value::as_str) {
                if let Some(otid) = otid_from_url(url) {
                    add_source(
                        sources,
                        SourceInfo {
                            title: String::new(),
                            otid,
                            speech_id: String::new(),
                            start_time: String::new(),
                        },
                    );
                }
            }
            if let Some(children) = map.get("children") {
                collect_sources_from_urls(children, sources);
            }
            if let Some(content) = map.get("content") {
                collect_sources_from_urls(content, sources);
            }
        }
        _ => {}
    }
}

// legacy helper no longer used; kept out to satisfy old references

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
            {"type":"text_block","children":[{"type":"text","text":"Hello world"},{"type":"text","text":"from Otter"}]},
            {"type":"list_block","children":[
                {"type":"list_item","children":[{"type":"text_block","children":[{"type":"text","text":"One"}]}]},
                {"type":"list_item","children":[{"type":"text_block","children":[{"type":"text","text":"Two"}]}]}
            ]}
        ]);
        let mut sources = Vec::new();
        let text = render_blocks(&blocks, &mut sources);
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
        let mut sources = Vec::new();
        collect_sources_from_urls(&blocks, &mut sources);
        assert!(sources.iter().any(|s| s.otid == "abc123"));
        assert!(!sources.iter().any(|s| s.otid == "def456"));
    }

    #[test]
    fn render_fixture_lists_and_sources() {
        let value: Value =
            serde_json::from_str(include_str!("../tests/fixtures/ask_sample.json")).unwrap();
        let blocks = &value["blocks"];
        let mut sources = Vec::new();
        let text = render_blocks(blocks, &mut sources);
        // Bullet main lines
        assert!(text.contains("- First Meeting [TESTOTID1]"));
        assert!(text.contains("- Second Meeting [TESTOTID1]"));
        // Sub-lines begin on their own lines with consistent indentation and no stray spaces
        assert!(text.contains("\n  - Date: Thu Sep 24, 2026"));
        assert!(text.contains("\n  - Participants: Alex Example, Bob Example"));
        // No stray "- --" lines; divider rendered as --- or skipped
        assert!(!text.contains("- --"));
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].otid, "TESTOTID1");
    }
}
