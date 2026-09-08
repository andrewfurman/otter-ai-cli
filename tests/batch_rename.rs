use std::process::Command;

fn run_plan(plan: &str, flags: &[&str]) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plan.json");
    std::fs::write(&file, plan).unwrap();
    Command::new(env!("CARGO_BIN_EXE_otter"))
        .env_clear()
        .env("HOME", dir.path())
        .args(["speeches", "rename-batch"])
        .arg(file)
        .args(flags)
        .output()
        .unwrap()
}

#[test]
fn preview_is_offline_and_json_preserves_exact_titles() {
    let output = run_plan(
        r#"[{"otid":"a","old_title":null,"new_title":"Café & QA"},{"otid":"b","old_title":"old","new_title":"new"}]"#,
        &["--dry-run", "--json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["status"], "preview");
    assert_eq!(data["count"], 2);
    assert!(data["changes"][0]["old_title"].is_null());
    assert_eq!(data["changes"][0]["new_title"], "Café & QA");
}

#[test]
fn invalid_later_entry_fails_before_authentication_even_without_dry_run() {
    let output = run_plan(
        r#"[{"otid":"a","old_title":"old","new_title":"new"},{"otid":"b","old_title":"old","new_title":" "}]"#,
        &[],
    );
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("new_title for b must not be blank"));
    assert!(!error.contains("Not logged in"));
    assert!(output.stdout.is_empty());
}
