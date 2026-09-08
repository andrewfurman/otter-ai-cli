//! Opt-in, read-only smoke test against the real Otter.ai API.
//! Run explicitly with: cargo test --test live -- --ignored --nocapture

use otter::{ApiResponse, Client, Error};
use serde_json::Value;

fn required_credential(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("Set {name} before explicitly running the live smoke test"))
}

fn checked(stage: &str, response: Result<ApiResponse, Error>) -> Value {
    // Never print raw responses, credentials, cookies, or request URLs.
    let response = response.unwrap_or_else(|_| {
        panic!("{stage} failed with a transport or response error; stopping live smoke test")
    });
    assert!(
        response.ok(),
        "{stage} failed (HTTP {}; retry_after_seconds={:?}); stopping without retries",
        response.status,
        response.retry_after_seconds,
    );
    response.data
}

#[test]
#[ignore = "live Otter API: opt in with --ignored and set OTTERAI_USERNAME/OTTERAI_PASSWORD"]
fn live_smoke() {
    // Both credentials are validated before creating a client or sending requests.
    let username = required_credential("OTTERAI_USERNAME");
    let password = required_credential("OTTERAI_PASSWORD");
    let mut client = Client::new().expect("create live smoke client");
    let login = checked("login", client.login(&username, &password));
    assert!(
        login["email"]
            .as_str()
            .is_some_and(|email| email.eq_ignore_ascii_case(&username)),
        "Login response did not identify the expected account"
    );
    drop(password);
    drop(username);

    // All checks run sequentially through this single authenticated client.
    checked("user", client.get_user());
    let speeches = checked("speeches", client.get_speeches("0", 5, "owned"));
    assert!(speeches["speeches"].is_array(), "speeches array missing");
    let folders = checked("folders", client.get_folders());
    assert!(folders["folders"].is_array(), "folders array missing");
    let speakers = checked("speakers", client.get_speakers());
    assert!(speakers["speakers"].is_array(), "speakers array missing");
    checked("groups", client.list_groups());

    eprintln!("Live smoke passed: login, user, speeches, folders, speakers, groups; one authenticated session.");
}
