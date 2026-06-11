//! Live tests against the real Otter.ai API, gated on credentials in the
//! environment like the Python suite (OTTERAI_USERNAME / OTTERAI_PASSWORD).

use otterai::Client;

fn live_client() -> Option<Client> {
    let (Ok(username), Ok(password)) = (
        std::env::var("OTTERAI_USERNAME"),
        std::env::var("OTTERAI_PASSWORD"),
    ) else {
        eprintln!("skipping live test: OTTERAI_USERNAME/OTTERAI_PASSWORD not set");
        return None;
    };
    let mut client = Client::new().unwrap();
    let result = client.login(&username, &password).unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.data["email"].as_str(), Some(username.as_str()));
    Some(client)
}

#[test]
fn live_login() {
    let Some(_client) = live_client() else { return };
}

#[test]
fn live_get_speeches() {
    let Some(client) = live_client() else { return };
    let result = client.get_speeches("0", 5, "owned").unwrap();
    assert_eq!(result.status, 200);
    assert!(result.data["speeches"].is_array());
}

#[test]
fn live_get_folders_speakers_groups() {
    let Some(client) = live_client() else { return };

    let folders = client.get_folders().unwrap();
    assert_eq!(folders.status, 200);
    assert!(folders.data["folders"].is_array());

    let speakers = client.get_speakers().unwrap();
    assert_eq!(speakers.status, 200);
    assert!(speakers.data["speakers"].is_array());

    let groups = client.list_groups().unwrap();
    assert_eq!(groups.status, 200);
}

#[test]
fn live_get_user() {
    let Some(client) = live_client() else { return };
    let result = client.get_user().unwrap();
    assert_eq!(result.status, 200);
}
