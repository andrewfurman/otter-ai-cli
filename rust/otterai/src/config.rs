//! Credential storage compatible with the Python CLI: ~/.otterai/config.json,
//! overridable with OTTERAI_USERNAME / OTTERAI_PASSWORD.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct StoredConfig {
    username: Option<String>,
    password: Option<String>,
}

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".otterai")
        .join("config.json")
}

/// Env vars take precedence (both must be set), then the config file.
pub fn load_credentials() -> (Option<String>, Option<String>) {
    let env_user = std::env::var("OTTERAI_USERNAME")
        .ok()
        .filter(|s| !s.is_empty());
    let env_pass = std::env::var("OTTERAI_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());
    if let (Some(u), Some(p)) = (env_user, env_pass) {
        return (Some(u), Some(p));
    }
    load_credentials_from(&config_path())
}

fn load_credentials_from(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, None);
    };
    match serde_json::from_str::<StoredConfig>(&text) {
        Ok(config) => (config.username, config.password),
        Err(_) => (None, None),
    }
}

pub fn save_credentials(username: &str, password: &str) -> io::Result<()> {
    save_credentials_to(&config_path(), username, password)
}

fn save_credentials_to(path: &Path, username: &str, password: &str) -> io::Result<()> {
    let dir = path.parent().expect("config path has a parent directory");
    fs::create_dir_all(dir)?;
    let config = StoredConfig {
        username: Some(username.to_string()),
        password: Some(password.to_string()),
    };
    fs::write(path, serde_json::to_string_pretty(&config)? + "\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Returns true if a config file existed and was removed.
pub fn clear_credentials() -> io::Result<bool> {
    let path = config_path();
    if path.exists() {
        fs::remove_file(path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".otterai").join("config.json");
        save_credentials_to(&path, "user@example.com", "secret").unwrap();
        let (username, password) = load_credentials_from(&path);
        assert_eq!(username.as_deref(), Some("user@example.com"));
        assert_eq!(password.as_deref(), Some("secret"));
    }

    #[test]
    fn missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let (username, password) = load_credentials_from(&dir.path().join("config.json"));
        assert!(username.is_none() && password.is_none());
    }

    #[test]
    fn invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "not json").unwrap();
        let (username, password) = load_credentials_from(&path);
        assert!(username.is_none() && password.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".otterai").join("config.json");
        save_credentials_to(&path, "u", "p").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
