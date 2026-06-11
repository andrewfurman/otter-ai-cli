use serde::Deserialize;

const API_BASE_URL: &str = "https://otter.ai/forward/api/v1/";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("login failed (status {status}): {body}")]
    LoginFailed { status: u16, body: String },
}

/// Fields of the login response the CLI cares about; the API returns more.
#[derive(Debug, Deserialize)]
pub struct LoginData {
    pub userid: serde_json::Value,
    #[serde(default)]
    pub email: Option<String>,
}

pub struct Client {
    http: reqwest::blocking::Client,
    userid: Option<String>,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        // The cookie store keeps the session + csrftoken cookies that every
        // later endpoint depends on, like requests.Session in the Python client.
        let http = reqwest::blocking::Client::builder()
            .cookie_store(true)
            .build()?;
        Ok(Self { http, userid: None })
    }

    /// GET /login with HTTP Basic auth; the username is also passed as a query param.
    pub fn login(&mut self, username: &str, password: &str) -> Result<LoginData, Error> {
        let response = self
            .http
            .get(format!("{API_BASE_URL}login"))
            .query(&[("username", username)])
            .basic_auth(username, Some(password))
            .send()?;

        let status = response.status();
        if !status.is_success() {
            return Err(Error::LoginFailed {
                status: status.as_u16(),
                body: response.text().unwrap_or_default(),
            });
        }

        let data: LoginData = response.json()?;
        self.userid = Some(data.userid.to_string());
        Ok(data)
    }

    pub fn userid(&self) -> Option<&str> {
        self.userid.as_deref()
    }
}
