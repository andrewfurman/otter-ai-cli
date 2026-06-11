//! Live tests against the real Otter.ai API, gated on credentials in the
//! environment like the Python suite (OTTERAI_USERNAME / OTTERAI_PASSWORD).

use otterai::Client;

#[test]
fn live_login() {
    let (Ok(username), Ok(password)) = (
        std::env::var("OTTERAI_USERNAME"),
        std::env::var("OTTERAI_PASSWORD"),
    ) else {
        eprintln!("skipping live_login: OTTERAI_USERNAME/OTTERAI_PASSWORD not set");
        return;
    };

    let mut client = Client::new().unwrap();
    let data = client.login(&username, &password).unwrap();
    assert!(client.userid().is_some());
    assert_eq!(data.email.as_deref(), Some(username.as_str()));
}
