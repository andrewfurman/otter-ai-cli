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
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("userid is invalid")]
    InvalidUserId,
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
}

impl ApiResponse {
    pub fn ok(&self) -> bool {
        self.status == 200
    }
}

fn handle_response(response: reqwest::blocking::Response) -> Result<ApiResponse, Error> {
    let status = response.status().as_u16();
    // Like the Python client: a non-JSON body becomes an empty data dict.
    let data = response.json().unwrap_or_else(|_| json!({}));
    Ok(ApiResponse { status, data })
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
        let response = self
            .http
            .get(format!("{API_BASE_URL}speeches"))
            .query(&[
                ("userid", self.userid()?),
                ("folder", folder),
                ("page_size", &page_size.to_string()),
                ("source", source),
            ])
            .send()?;
        handle_response(response)
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
        if response.status().as_u16() != 200 {
            return handle_response(response);
        }
        let params: Value = response.json()?;
        let Some(fields) = params["data"].as_object() else {
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
        let bytes = std::fs::read(file_name)?;
        let part = multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(content_type)?;
        form = form.part("file", part);

        let response = self.http.post(S3_UPLOAD_URL).multipart(form).send()?;
        if response.status().as_u16() != 201 {
            return handle_response(response);
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

    /// Downloads to `<name or speech_id>.<ext>` and returns `{"filename": ...}` as data.
    pub fn download_speech(
        &self,
        speech_id: &str,
        name: Option<&str>,
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

        let extension = if fileformat.contains(',') {
            "zip"
        } else {
            fileformat
        };
        let filename = format!("{}.{extension}", name.unwrap_or(speech_id));
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(Error::Download {
                status,
                speech_id: speech_id.to_string(),
            });
        }
        std::fs::write(&filename, response.bytes()?)?;
        let mut data = Map::new();
        data.insert("filename".into(), Value::String(filename));
        Ok(ApiResponse {
            status,
            data: Value::Object(data),
        })
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
        let form: Vec<(&str, &str)> = speech_ids
            .iter()
            .map(|id| ("speech_otid_list", id.as_str()))
            .collect();
        let response = self
            .http
            .post(format!("{API_BASE_URL}add_folder_speeches"))
            .query(&[("userid", self.userid()?), ("folder_id", folder_id)])
            .header("x-csrftoken", self.csrf_token())
            .header("referer", "https://otter.ai/")
            .form(&form)
            .send()?;
        handle_response(response)
    }
}

fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

#[cfg(test)]
mod tests {
    use super::xml_tag;

    #[test]
    fn xml_tag_extracts_s3_fields() {
        let xml = "<PostResponse><Location>l</Location><Bucket>speech-upload-prod</Bucket><Key>k/v.wav</Key></PostResponse>";
        assert_eq!(xml_tag(xml, "Bucket"), Some("speech-upload-prod"));
        assert_eq!(xml_tag(xml, "Key"), Some("k/v.wav"));
        assert_eq!(xml_tag(xml, "ETag"), None);
    }
}
