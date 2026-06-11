use crate::auth::authenticated_client;
use crate::util::{api, fail, print_json, result_repr, value_str};

pub fn list(as_json: bool) {
    let client = authenticated_client();
    let result = api(client.get_folders());
    if !result.ok() {
        fail(format!("Failed to get folders: {}", result_repr(&result)));
    }

    if as_json {
        print_json(&result.data);
        return;
    }

    let folders = result.data["folders"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if folders.is_empty() {
        println!("No folders found.");
        return;
    }

    println!("Found {} folders:\n", folders.len());
    for folder in &folders {
        let name = match value_str(&folder["folder_name"]) {
            n if n.is_empty() => "Untitled".to_string(),
            n => n,
        };
        let count = folder["speech_count"].as_i64().unwrap_or(0);
        println!("  {}  {name} ({count} speeches)", value_str(&folder["id"]));
    }
}

pub fn create(name: String, as_json: bool) {
    let client = authenticated_client();
    let result = api(client.create_folder(&name));
    if !result.ok() {
        fail(format!("Failed to create folder: {}", result_repr(&result)));
    }

    if as_json {
        print_json(&result.data);
    } else {
        let id = match value_str(&result.data["folder"]["id"]) {
            i if i.is_empty() => "unknown".to_string(),
            i => i,
        };
        println!("Created folder '{name}' (ID: {id})");
    }
}

pub fn rename(folder_id: String, new_name: String) {
    let client = authenticated_client();
    let result = api(client.rename_folder(&folder_id, &new_name));
    if !result.ok() {
        fail(format!("Failed to rename folder: {}", result_repr(&result)));
    }
    println!("Renamed folder {folder_id} to '{new_name}'");
}
