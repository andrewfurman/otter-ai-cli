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
    format!("{{'status': {}, 'data': {}}}", result.status, result.data)
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
