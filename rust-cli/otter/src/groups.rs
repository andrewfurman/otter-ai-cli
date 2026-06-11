use serde_json::Value;

use crate::auth::authenticated_client;
use crate::util::{api, fail, print_json, result_repr, value_str};

pub fn list(as_json: bool) {
    let client = authenticated_client();
    let result = api(client.list_groups());
    if !result.ok() {
        fail(format!("Failed to get groups: {}", result_repr(&result)));
    }

    let data = &result.data;
    if as_json {
        print_json(data);
        return;
    }

    let Some(groups) = data.as_array() else {
        print_json(data);
        return;
    };
    if groups.is_empty() {
        println!("No groups found.");
        return;
    }

    println!("Found {} groups:", groups.len());
    for (idx, group) in groups.iter().enumerate() {
        let idx = idx + 1;
        match group {
            Value::Object(fields) => {
                let name = ["name", "group_name"]
                    .iter()
                    .map(|k| value_str(&fields.get(*k).cloned().unwrap_or(Value::Null)))
                    .find(|n| !n.is_empty())
                    .unwrap_or_else(|| "Unknown".to_string());
                match fields.get("id") {
                    Some(id) if !id.is_null() => {
                        println!("  {idx}. {name} (id: {})", value_str(id))
                    }
                    _ => println!("  {idx}. {name}"),
                }
            }
            other => println!("  {idx}. {}", value_str(other)),
        }
    }
}
