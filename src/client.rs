use std::sync::Arc;

use reqwest::blocking::multipart;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::Method;
use serde_json::{json, Map, Value};

const API_BASE_URL: &str = "https://otter.ai/forward/api/v1/";
const S3_UPLOAD_URL: &str = "https://s3.us-west-2.amazonaws.com/speech-upload-prod";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid JSON response (HTTP {status}): {source}")]
    InvalidJson {
        status: u16,
        #[source]
        source: reqwest::Error,
    },
    #[error("unexpected response (HTTP {status}): expected {expected}")]
    UnexpectedResponse { status: u16, expected: &'static str },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("userid is invalid")]
    InvalidUserId,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("Got response status {status} when attempting to download {speech_id}")]
    Download { status: u16, speech_id: String },
    #[error("upload failed: {0}")]
    Upload(String),
}

/// Mirror of the Python client's `{"status": ..., "data": ...}` response dicts.
#[derive(Debug)]
pub struct ApiResponse {
    pub status: u16,
    pub data: Value,
    /// Server-requested delay on HTTP 429, from Retry-After or JSON retry_after.
    pub retry_after_seconds: Option<u64>,
}

impl ApiResponse {
    /// Some endpoints return bare objects/arrays. When an API status is present,
    /// only OK acknowledges success, regardless of the HTTP status code.
    pub fn ok(&self) -> bool {
        self.status == 200
            && self.data.get("status").is_none_or(|status| {
                status
                    .as_str()
                    .is_some_and(|status| status.eq_ignore_ascii_case("OK"))
            })
    }
}

fn handle_response(response: reqwest::blocking::Response) -> Result<ApiResponse, Error> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let data = match response.json::<Value>() {
        Ok(data) => data,
        // Keep the HTTP failure and Retry-After even if an error page is HTML.
        Err(_) if status != 200 => json!({}),
        Err(source) => return Err(Error::InvalidJson { status, source }),
    };
    if status == 200 && !data.is_object() && !data.is_array() {
        return Err(Error::UnexpectedResponse {
            status,
            expected: "a JSON object or array",
        });
    }
    let retry_after_seconds = if status == 429 {
        retry_delay(retry_after.as_deref(), &data, std::time::SystemTime::now())
    } else {
        None
    };
    Ok(ApiResponse {
        status,
        data,
        retry_after_seconds,
    })
}

fn retry_delay(header: Option<&str>, data: &Value, now: std::time::SystemTime) -> Option<u64> {
    let from_header = header.and_then(|value| {
        let value = value.trim();
        value.parse::<u64>().ok().or_else(|| {
            let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
            let now = now.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            Some((date.timestamp().max(0) as u64).saturating_sub(now))
        })
    });
    let from_body = data["retry_after"]
        .as_u64()
        .or_else(|| data["retry_after"].as_str()?.trim().parse::<u64>().ok());
    // If both are present, honor the longer delay.
    from_header.into_iter().chain(from_body).max()
}

pub struct Client {
    http: reqwest::blocking::Client,
    jar: Arc<Jar>,
    userid: Option<String>,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        let jar = Arc::new(Jar::default());
        // The cookie store keeps the session + csrftoken cookies that every
        // later endpoint depends on, like requests.Session in the Python client.
        let http = reqwest::blocking::Client::builder()
            .cookie_provider(jar.clone())
            .build()?;
        Ok(Self {
            http,
            jar,
            userid: None,
        })
    }

    fn userid(&self) -> Result<&str, Error> {
        self.userid.as_deref().ok_or(Error::InvalidUserId)
    }

    fn csrf_token(&self) -> String {
        let url = "https://otter.ai/".parse().expect("static url parses");
        let Some(header) = self.jar.cookies(&url) else {
            return String::new();
        };
        header
            .to_str()
            .unwrap_or_default()
            .split("; ")
            .find_map(|cookie| cookie.strip_prefix("csrftoken="))
            .unwrap_or_default()
            .to_string()
    }

    /// GET /login with HTTP Basic auth; the username is also passed as a query param.
    pub fn login(&mut self, username: &str, password: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}login"))
            .query(&[("username", username)])
            .basic_auth(username, Some(password))
            .send()?;

        let result = handle_response(response)?;
        if result.ok() {
            self.userid = Some(match &result.data["userid"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
        Ok(result)
    }

    pub fn get_user(&self) -> Result<ApiResponse, Error> {
        let response = self.http.get(format!("{API_BASE_URL}user")).send()?;
        handle_response(response)
    }

    pub fn get_speakers(&self) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}speakers"))
            .query(&[("userid", self.userid()?)])
            .send()?;
        handle_response(response)
    }

    pub fn get_speeches(
        &self,
        folder: &str,
        page_size: u32,
        source: &str,
    ) -> Result<ApiResponse, Error> {
        self.get_speeches_page(folder, page_size, source, None)
    }

    pub fn get_speeches_page(
        &self,
        folder: &str,
        page_size: u32,
        source: &str,
        last_load_ts: Option<u64>,
    ) -> Result<ApiResponse, Error> {
        let response = self
            .speeches_request(folder, page_size, source, last_load_ts)?
            .send()?;
        handle_response(response)
    }

    fn speeches_request(
        &self,
        folder: &str,
        page_size: u32,
        source: &str,
        last_load_ts: Option<u64>,
    ) -> Result<reqwest::blocking::RequestBuilder, Error> {
        let mut request = self.http.get(format!("{API_BASE_URL}speeches")).query(&[
            ("userid", self.userid()?),
            ("folder", folder),
            ("page_size", &page_size.to_string()),
            ("source", source),
        ]);
        if let Some(cursor) = last_load_ts {
            // The unofficial endpoint requires a positive modified_after with
            // the cursor. It can repeat recently modified recordings on pages.
            request = request.query(&[("last_load_ts", cursor), ("modified_after", 1)]);
        }
        Ok(request)
    }

    pub fn get_speech(&self, speech_id: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}speech"))
            .query(&[("userid", self.userid()?), ("otid", speech_id)])
            .send()?;
        handle_response(response)
    }

    pub fn set_speech_title(&self, speech_id: &str, title: &str) -> Result<ApiResponse, Error> {
        self.userid()?;
        let response = self
            .http
            .get(format!("{API_BASE_URL}set_speech_title"))
            .query(&[("otid", speech_id), ("title", title)])
            .send()?;
        handle_response(response)
    }

    /// Search a speech via GET `advanced_search`.
    /// Speaker filtering is applied client-side; Otter has no documented
    /// list/search query parameter for speaker name or id.
    pub fn query_speech(
        &self,
        query: &str,
        speech_id: &str,
        size: u32,
    ) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}advanced_search"))
            .query(&[
                ("query", query),
                ("size", &size.to_string()),
                ("otid", speech_id),
            ])
            .send()?;
        handle_response(response)
    }

    pub fn upload_speech(&self, file_name: &str, content_type: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}speech_upload_params"))
            .query(&[("userid", self.userid()?)])
            .send()?;
        let params = handle_response(response)?;
        if !params.ok() {
            return Ok(params);
        }
        let Some(fields) = params.data["data"].as_object() else {
            return Err(Error::Upload(
                "speech_upload_params returned no data".into(),
            ));
        };

        // CORS preflight, exactly as the browser (and Python client) sends it.
        let response = self
            .http
            .request(Method::OPTIONS, S3_UPLOAD_URL)
            .header("Accept", "*/*")
            .header("Connection", "keep-alive")
            .header("Origin", "https://otter.ai")
            .header("Referer", "https://otter.ai/")
            .header("Access-Control-Request-Method", "POST")
            .send()?;
        if response.status().as_u16() != 200 {
            return handle_response(response);
        }

        // S3 POST policy: all signed fields first, the file part last.
        let mut form = multipart::Form::new();
        for (key, value) in fields {
            if key == "form_action" {
                continue;
            }
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            form = form.text(key.clone(), text);
        }
        let part = multipart::Part::file(file_name)?
            .file_name(file_name.to_string())
            .mime_str(content_type)?;
        form = form.part("file", part);

        let response = self.http.post(S3_UPLOAD_URL).multipart(form).send()?;
        if response.status().as_u16() != 201 {
            let result = handle_response(response)?;
            if result.ok() {
                return Err(Error::Upload(
                    "S3 did not acknowledge the upload with HTTP 201".into(),
                ));
            }
            return Ok(result);
        }
        let xml = response.text()?;
        let bucket = xml_tag(&xml, "Bucket")
            .ok_or_else(|| Error::Upload(format!("no Bucket in S3 response: {xml}")))?;
        let key = xml_tag(&xml, "Key")
            .ok_or_else(|| Error::Upload(format!("no Key in S3 response: {xml}")))?;

        let response = self
            .http
            .get(format!("{API_BASE_URL}finish_speech_upload"))
            .query(&[
                ("bucket", bucket),
                ("key", key),
                ("language", "en"),
                ("country", "us"),
                ("userid", self.userid()?),
                // Required since mid-2026; the API rejects the finish call without it.
                ("appid", "otter-web"),
            ])
            .send()?;
        handle_response(response)
    }

    /// Downloads to the exact output path, defaulting to `<speech_id>.<ext>`.
    pub fn download_speech(
        &self,
        speech_id: &str,
        output: Option<&str>,
        fileformat: &str,
    ) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .post(format!("{API_BASE_URL}bulk_export"))
            .query(&[("userid", self.userid()?)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("formats", fileformat), ("speech_otid_list", speech_id)])
            .send()?;

        let filename = export_filename(speech_id, output, fileformat);
        save_export(response, &filename)
    }

    pub fn move_to_trash_bin(&self, speech_id: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .post(format!("{API_BASE_URL}move_to_trash_bin"))
            .query(&[("userid", self.userid()?)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("otid", speech_id)])
            .send()?;
        handle_response(response)
    }

    pub fn create_speaker(&self, speaker_name: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .post(format!("{API_BASE_URL}create_speaker"))
            .query(&[("userid", self.userid()?)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("speaker_name", speaker_name)])
            .send()?;
        handle_response(response)
    }

    pub fn set_transcript_speaker(
        &self,
        speech_id: &str,
        transcript_uuid: &str,
        speaker_id: &str,
        speaker_name: &str,
        create_speaker: bool,
    ) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}set_transcript_speaker"))
            .query(&[
                ("speech_otid", speech_id),
                ("transcript_uuid", transcript_uuid),
                ("speaker_name", speaker_name),
                ("userid", self.userid()?),
                (
                    "create_speaker",
                    if create_speaker { "true" } else { "false" },
                ),
                ("speaker_id", speaker_id),
            ])
            .header("referer", "https://otter.ai/")
            .header("x-csrftoken", self.csrf_token())
            .send()?;
        handle_response(response)
    }

    pub fn list_groups(&self) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}list_groups"))
            .query(&[("userid", self.userid()?)])
            .send()?;
        handle_response(response)
    }

    pub fn get_folders(&self) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}folders"))
            .query(&[("userid", self.userid()?)])
            .send()?;
        handle_response(response)
    }

    pub fn create_folder(&self, folder_name: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .post(format!("{API_BASE_URL}create_folder"))
            .query(&[("userid", self.userid()?)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("folder_name", folder_name)])
            .send()?;
        handle_response(response)
    }

    pub fn rename_folder(&self, folder_id: &str, new_name: &str) -> Result<ApiResponse, Error> {
        let response = self
            .http
            .post(format!("{API_BASE_URL}rename_folder"))
            .query(&[("userid", self.userid()?), ("folder_id", folder_id)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("new_name", new_name)])
            .send()?;
        handle_response(response)
    }

    pub fn add_folder_speeches(
        &self,
        folder_id: &str,
        speech_ids: &[String],
    ) -> Result<ApiResponse, Error> {
        let response = self.move_request(folder_id, speech_ids)?.send()?;
        handle_response(response)
    }

    fn move_request(
        &self,
        folder_id: &str,
        speech_ids: &[String],
    ) -> Result<reqwest::blocking::RequestBuilder, Error> {
        if speech_ids.is_empty()
            || speech_ids
                .iter()
                .any(|id| id.trim().is_empty() || id.contains(','))
        {
            return Err(Error::InvalidInput(
                "provide nonempty OTIDs separately, without commas".into(),
            ));
        }
        // Otter reads one field, not repeated form keys (which silently move only
        // the last recording). Keep the comma inside the form-encoded value.
        Ok(self
            .http
            .post(format!("{API_BASE_URL}add_folder_speeches"))
            .query(&[("userid", self.userid()?), ("folder_id", folder_id)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&[("speech_otid_list", speech_ids.join(","))]))
    }
}

fn export_filename(speech_id: &str, output: Option<&str>, format: &str) -> String {
    output.map(str::to_owned).unwrap_or_else(|| {
        let extension = if format.contains(',') { "zip" } else { format };
        format!("{speech_id}.{extension}")
    })
}

fn save_export(
    mut response: reqwest::blocking::Response,
    filename: &str,
) -> Result<ApiResponse, Error> {
    // Preserve status and retry guidance regardless of an error's Content-Type.
    // A partial (206) response must not replace the requested complete export.
    if response.status().as_u16() != 200 {
        return handle_response(response);
    }
    // Export formats are files, but Otter may instead send a JSON rejection
    // with HTTP 200. Never save that error as the requested output file.
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_json_content_type)
    {
        let result = handle_response(response)?;
        if !result.ok() {
            return Ok(result);
        }
        return Err(Error::UnexpectedResponse {
            status: result.status,
            expected: "an export file, not JSON",
        });
    }

    let status = response.status().as_u16();
    let destination = std::path::Path::new(filename);
    let directory = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    // The temporary file is on the destination filesystem, so a completed
    // transfer can replace it atomically. Errors drop the temporary file.
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    response.copy_to(temporary.as_file_mut())?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    let mut data = Map::new();
    data.insert("filename".into(), Value::String(filename.to_string()));
    Ok(ApiResponse {
        status,
        data: Value::Object(data),
        retry_after_seconds: None,
    })
}

fn is_json_content_type(value: &str) -> bool {
    let media_type = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

/// Case-insensitive substring on `speaker_name`, or exact match on leftover
/// id keys (`speaker_id`, `id`). Needle is trimmed. Used for client-side
/// `--speaker` filtering (no API query param exists).
pub fn speaker_matches(value: &Value, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }

    if let Some(name) = value.get("speaker_name") {
        let name = json_as_str(name);
        if !name.is_empty() && name.to_lowercase().contains(&needle.to_lowercase()) {
            return true;
        }
    }

    for key in ["speaker_id", "id"] {
        if let Some(id) = value.get(key) {
            if json_as_str(id) == needle {
                return true;
            }
        }
    }

    false
}

fn json_as_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

#[cfg(test)]
mod tests {
    use super::{
        export_filename, handle_response, retry_delay, save_export, speaker_matches, xml_tag, Error,
    };
    use serde_json::json;

    #[test]
    fn bulk_move_request_sends_one_comma_separated_field() {
        let mut client = super::Client::new().unwrap();
        client.userid = Some("123".into());
        let request = client
            .move_request("456", &["first-OTID".into(), "second_OTID".into()])
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().path(), "/forward/api/v1/add_folder_speeches");
        assert_eq!(request.url().query(), Some("userid=123&folder_id=456"));
        assert_eq!(
            request.headers()["content-type"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            b"speech_otid_list=first-OTID%2Csecond_OTID"
        );
        for ids in [vec![], vec!["".into()], vec!["first,second".into()]] {
            assert!(matches!(
                client.move_request("456", &ids),
                Err(Error::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn listing_requests_keep_filters_and_add_both_cursor_parameters() {
        let mut client = super::Client::new().unwrap();
        client.userid = Some("123".into());
        for (cursor, expected) in [
            (None, "userid=123&folder=456&page_size=100&source=shared"),
            (Some(987), "userid=123&folder=456&page_size=100&source=shared&last_load_ts=987&modified_after=1"),
        ] {
            let request = client.speeches_request("456", 100, "shared", cursor).unwrap().build().unwrap();
            assert_eq!(request.method(), reqwest::Method::GET);
            assert_eq!(request.url().path(), "/forward/api/v1/speeches");
            assert_eq!(request.url().query(), Some(expected));
        }
    }

    #[test]
    fn retry_delays_accept_headers_json_and_http_dates() {
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1672531200);
        assert_eq!(
            retry_delay(Some("30"), &json!({"retry_after": 16}), now),
            Some(30)
        );
        assert_eq!(
            retry_delay(Some("5"), &json!({"retry_after": "16"}), now),
            Some(16)
        );
        assert_eq!(
            retry_delay(Some("Sun, 01 Jan 2023 00:01:00 GMT"), &json!({}), now),
            Some(60)
        );
        assert_eq!(
            retry_delay(Some("Sat, 31 Dec 2022 23:59:00 GMT"), &json!({}), now),
            Some(0)
        );
        assert_eq!(
            retry_delay(Some("invalid"), &json!({"retry_after": -1}), now),
            None
        );
        assert_eq!(
            retry_delay(None, &json!({"retry_after": 16}), now),
            Some(16)
        );
        assert_eq!(retry_delay(None, &json!({}), now), None);
    }

    fn mock_response(status: u16, headers: &str, body: &str) -> reqwest::blocking::Response {
        mock_response_with_length(status, headers, body, body.len())
    }

    fn mock_response_with_length(
        status: u16,
        headers: &str,
        body: &str,
        length: usize,
    ) -> reqwest::blocking::Response {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = format!("HTTP/1.1 {status} Fixture\r\n{headers}Content-Length: {length}\r\nConnection: close\r\n\r\n{body}");
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before its headers");
                request.extend_from_slice(&buffer[..count]);
            }
            socket.write_all(reply.as_bytes()).unwrap();
        });
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client.get(format!("http://{address}/")).send().unwrap();
        server.join().unwrap();
        response
    }

    #[test]
    fn http_response_preserves_rate_limit_header_and_body() {
        let response = mock_response(
            429,
            "Retry-After: 30\r\n",
            r#"{"status":"failed","retry_after":16}"#,
        );
        let result = handle_response(response).unwrap();
        assert_eq!(result.status, 429);
        assert_eq!(result.retry_after_seconds, Some(30));
        assert_eq!(result.data["retry_after"], 16);
    }

    #[test]
    fn api_status_and_legacy_payloads_are_classified_consistently() {
        for (status, data, expected) in [
            (200, json!({"status": "OK"}), true),
            (200, json!({"status": "ok"}), true),
            (
                200,
                json!({"status": "failed", "message": "permission denied"}),
                false,
            ),
            (200, json!({"status": "error"}), false),
            (200, json!({"status": "pending"}), false),
            (200, json!({"status": false}), false),
            (200, json!({"status": null}), false),
            (200, json!({"results": []}), true),
            (200, json!([{"id": 7, "group_name": "Team"}]), true),
            (403, json!({"status": "OK"}), false),
            (500, json!({"status": "failed"}), false),
        ] {
            let result = handle_response(mock_response(status, "", &data.to_string())).unwrap();
            assert_eq!(result.ok(), expected, "HTTP {status}: {data}");
            assert_eq!(
                result.data, data,
                "keep server details for command error messages"
            );
        }
    }

    #[test]
    fn malformed_success_responses_are_errors() {
        for body in ["", "<html>upstream error</html>", "{"] {
            assert!(matches!(
                handle_response(mock_response(200, "", body)),
                Err(Error::InvalidJson { status: 200, .. })
            ));
        }
        for body in ["null", "42", "\"unexpected scalar\""] {
            assert!(matches!(
                handle_response(mock_response(200, "", body)),
                Err(Error::UnexpectedResponse { status: 200, .. })
            ));
        }
    }

    #[test]
    fn non_json_http_errors_preserve_status_and_retry_headers() {
        for status in [403, 429, 500] {
            let response = mock_response(status, "Retry-After: 30\r\n", "<html>error</html>");
            let result = handle_response(response).unwrap();
            assert!(!result.ok());
            assert_eq!(result.status, status);
            assert_eq!(
                result.retry_after_seconds,
                if status == 429 { Some(30) } else { None }
            );
        }
    }

    #[test]
    fn json_export_errors_do_not_overwrite_files() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("existing.txt");
        std::fs::write(&filename, "keep existing content").unwrap();
        for (content_type, body) in [
            (
                "application/json",
                r#"{"status":"failed","message":"permission denied"}"#,
            ),
            (
                "application/problem+json; charset=utf-8",
                r#"{"status":"error"}"#,
            ),
            ("Application/JSON", "<html>invalid JSON</html>"),
            ("application/json", r#"{"status":"OK"}"#),
        ] {
            let response = mock_response(200, &format!("Content-Type: {content_type}\r\n"), body);
            let result = save_export(response, filename.to_str().unwrap());
            assert!(result.is_err() || !result.unwrap().ok());
            assert_eq!(
                std::fs::read_to_string(&filename).unwrap(),
                "keep existing content"
            );
        }
    }

    #[test]
    fn successful_exports_still_write_file_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("transcript.txt");
        // JSON-looking text is a legitimate transcript when served as a file.
        let response = mock_response(
            200,
            "Content-Type: text/plain\r\n",
            r#"{"a transcript can contain JSON": true}"#,
        );
        let result = save_export(response, filename.to_str().unwrap()).unwrap();
        assert!(result.ok());
        assert_eq!(result.data["filename"], filename.to_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(filename).unwrap(),
            r#"{"a transcript can contain JSON": true}"#
        );
    }

    #[test]
    fn export_paths_are_exact_when_provided_and_default_to_the_format() {
        for (output, format, expected) in [
            (Some("recording.mp3"), "mp3", "recording.mp3"),
            (Some("folder/transcript"), "txt", "folder/transcript"),
            (Some("folder/export.zip"), "txt,pdf", "folder/export.zip"),
            (Some("custom.bin"), "txt,pdf", "custom.bin"),
            (None, "mp3", "fixture.mp3"),
            (None, "txt,pdf", "fixture.zip"),
        ] {
            assert_eq!(export_filename("fixture", output, format), expected);
        }
    }

    #[test]
    fn export_http_errors_keep_retry_guidance_without_writing_files() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("output.txt");
        std::fs::write(&filename, "original").unwrap();
        for (status, content_type, body, delay) in [
            (
                429,
                "application/json",
                r#"{"status":"failed","retry_after":16}"#,
                Some(30),
            ),
            (429, "text/html", "<html>rate limited</html>", Some(30)),
            (403, "text/html", "<html>forbidden</html>", None),
            (500, "text/plain", "server error", None),
            (206, "text/plain", "partial transcript", None),
        ] {
            let response = mock_response(
                status,
                &format!("Content-Type: {content_type}\r\nRetry-After: 30\r\n"),
                body,
            );
            let result = save_export(response, filename.to_str().unwrap()).unwrap();
            assert!(!result.ok());
            assert_eq!(result.status, status);
            assert_eq!(result.retry_after_seconds, delay);
            assert_eq!(std::fs::read_to_string(&filename).unwrap(), "original");
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn interrupted_exports_preserve_the_destination_and_remove_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("output.txt");
        for existing in [false, true] {
            if existing {
                std::fs::write(&filename, "original").unwrap();
            }
            let response =
                mock_response_with_length(200, "Content-Type: text/plain\r\n", "partial", 100);
            assert!(save_export(response, filename.to_str().unwrap()).is_err());
            assert_eq!(filename.exists(), existing);
            assert_eq!(
                std::fs::read_dir(directory.path()).unwrap().count(),
                usize::from(existing)
            );
            if existing {
                assert_eq!(std::fs::read_to_string(&filename).unwrap(), "original");
            }
        }
    }

    #[test]
    fn completed_exports_replace_existing_files_and_clean_up_on_persist_failure() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("output.txt");
        std::fs::write(&filename, "original").unwrap();
        let response = mock_response(200, "Content-Type: text/plain\r\n", "complete");
        assert!(save_export(response, filename.to_str().unwrap())
            .unwrap()
            .ok());
        assert_eq!(std::fs::read_to_string(&filename).unwrap(), "complete");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);

        let blocked = directory.path().join("directory");
        std::fs::create_dir(&blocked).unwrap();
        let response = mock_response(200, "Content-Type: text/plain\r\n", "complete");
        assert!(save_export(response, blocked.to_str().unwrap()).is_err());
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn xml_tag_extracts_s3_fields() {
        let xml = "<PostResponse><Location>l</Location><Bucket>speech-upload-prod</Bucket><Key>k/v.wav</Key></PostResponse>";
        assert_eq!(xml_tag(xml, "Bucket"), Some("speech-upload-prod"));
        assert_eq!(xml_tag(xml, "Key"), Some("k/v.wav"));
        assert_eq!(xml_tag(xml, "ETag"), None);
    }

    #[test]
    fn speaker_matches_name_substring_case_insensitive() {
        let speaker = json!({"id": 1, "speaker_name": "Alice Example"});
        assert!(speaker_matches(&speaker, "alice"));
        assert!(speaker_matches(&speaker, "Alice"));
        assert!(speaker_matches(&speaker, "EXAMPLE"));
        assert!(speaker_matches(&speaker, "  ali  "));
        assert!(!speaker_matches(&speaker, "Bob"));
    }

    #[test]
    fn speaker_matches_id_exact_stringified() {
        let by_id = json!({"id": 99, "speaker_name": "Alice"});
        assert!(speaker_matches(&by_id, "99"));
        assert!(!speaker_matches(&by_id, "9"));

        let by_speaker_id = json!({"speaker_id": "42", "speaker_name": "Bob"});
        assert!(speaker_matches(&by_speaker_id, "42"));
        assert!(!speaker_matches(&by_speaker_id, "4"));
    }

    #[test]
    fn speaker_matches_empty_needle_is_false() {
        let speaker = json!({"id": 1, "speaker_name": "Alice"});
        assert!(!speaker_matches(&speaker, ""));
        assert!(!speaker_matches(&speaker, "   "));
    }
}
