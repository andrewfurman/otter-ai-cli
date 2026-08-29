use otter::speaker_matches;
use serde_json::json;

#[test]
fn speaker_matches_name_substring_and_exact_id() {
    let alice = json!({"id": 11, "speaker_name": "Alice Example"});
    assert!(speaker_matches(&alice, "alice"));
    assert!(speaker_matches(&alice, "Example"));
    assert!(speaker_matches(&alice, "11"));
    assert!(!speaker_matches(&alice, "1"));
    assert!(!speaker_matches(&alice, "Bob"));
}

#[test]
fn speaker_matches_speaker_id_field() {
    let bob = json!({"speaker_id": "42", "speaker_name": "Bob"});
    assert!(speaker_matches(&bob, "42"));
    assert!(speaker_matches(&bob, "bob"));
    assert!(!speaker_matches(&bob, "4"));
    assert!(!speaker_matches(&bob, ""));
}
