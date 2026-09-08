//! Help and argument validation must finish without authenticating.
use std::process::Command;

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_otter"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn both_root_help_forms_show_every_command_and_rate_guidance() {
    for args in [&["--help"][..], &["help"][..]] {
        let text = help(args);
        for command in [
            "login",
            "logout",
            "user",
            "speeches list",
            "speeches get",
            "speeches search",
            "speeches rename",
            "speeches download",
            "speeches upload",
            "speeches trash",
            "speeches move",
            "speakers list",
            "speakers create",
            "speakers tag",
            "folders list",
            "folders create",
            "folders rename",
            "groups list",
            "config show",
            "config clear",
        ] {
            assert!(
                text.contains(&format!("otter {command}")),
                "missing {command}"
            );
        }
        assert!(text.contains("HTTP 429"));
        assert!(text.contains("60-90 seconds"));
        assert!(text.contains("not an official quota"));
    }
}

#[test]
fn nested_tag_help_explains_batching_and_all_scope() {
    for args in [
        &["speakers", "tag", "--help"][..],
        &["help", "speakers", "tag"][..],
    ] {
        let text = help(args);
        assert!(text.contains("-t UUID1 -t UUID2"));
        assert!(text.contains("EVERY segment"));
        assert!(text.contains("one login"));
        assert!(text.contains("HTTP 429"));
    }
}

#[test]
fn conflicting_tag_flags_fail_before_authentication() {
    let output = Command::new(env!("CARGO_BIN_EXE_otter"))
        .args(["speakers", "tag", "otid", "42", "--all", "-t", "uuid"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot be used with"));
    assert!(!error.contains("Login failed"));
}
