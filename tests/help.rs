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
            "search",
            "speeches list",
            "speeches get",
            "speeches search",
            "speeches rename",
            "speeches rename-batch",
            "speeches download",
            "speeches upload",
            "speeches trash",
            "speeches move",
            "speakers list",
            "speakers create",
            "speakers tag",
            "speakers untag",
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
fn top_level_help_is_identical_for_every_form_and_has_examples() {
    let forms = [&["--help"][..], &["-h"][..], &["help"][..]];
    let texts: Vec<String> = forms.iter().map(|args| help(args)).collect();
    assert!(texts.iter().all(|t| *t == texts[0]));
    let text = &texts[0];
    for needle in [
        "otter login",
        "Examples:",
        "More help:",
        "Safety:",
        "--json",
        "Sign in & settings:",
        "Read (never changes anything):",
        "Change (",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert!(
        !text.contains("\nCommands:\n"),
        "command list is duplicated"
    );
    assert!(
        !text.contains("Other:"),
        "ungrouped command in top-level help"
    );
    assert!(
        text.lines().count() <= 70,
        "top-level help should fit a screen"
    );
}

#[test]
fn short_and_long_help_match_for_subcommands() {
    for path in [
        &["search"][..],
        &["speeches", "list"][..],
        &["speakers", "tag"][..],
    ] {
        let mut short: Vec<&str> = path.to_vec();
        short.push("-h");
        let mut long: Vec<&str> = path.to_vec();
        long.push("--help");
        let mut via_help: Vec<&str> = vec!["help"];
        via_help.extend(path);
        assert_eq!(help(&short), help(&long));
        assert_eq!(help(&long), help(&via_help));
    }
}

#[test]
fn mistakes_point_to_help_and_keep_exit_code_2() {
    for args in [
        &["search", "--bogus"][..],
        &["nonsense"][..],
        &["speeches", "get"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_otter"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("otter help <command>"), "{args:?}: {error}");
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
fn listing_move_and_export_help_describe_their_limits() {
    let list = help(&["speeches", "list", "--help"]);
    assert!(list.contains("--all"));
    assert!(list.contains("--max-pages"));
    assert!(list.contains("last N days"));
    assert!(list.contains("one page"));
    let download = help(&["speeches", "download", "--help"]);
    assert!(download.contains("Exact output path"));
    assert!(download.contains("OTID.zip"));
    let move_help = help(&["speeches", "move", "--help"]);
    assert!(move_help.contains("successful lookup"));

    let output = Command::new(env!("CARGO_BIN_EXE_otter"))
        .args(["speeches", "download", "fixture", "--output", ""])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8(output.stderr)
        .unwrap()
        .contains("Login failed"));
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

#[test]
fn invalid_listing_bounds_fail_before_authentication() {
    for (flag, value) in [("--days", "0"), ("--page-size", "0"), ("--max-pages", "0")] {
        let output = Command::new(env!("CARGO_BIN_EXE_otter"))
            .args(["speeches", "list", flag, value])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8(output.stderr)
            .unwrap()
            .contains("Login failed"));
    }
}

#[test]
fn batch_rename_help_explains_preview_conflicts_and_verification() {
    let text = help(&["speeches", "rename-batch", "--help"]);
    for phrase in [
        "old_title",
        "new_title",
        "--dry-run",
        "works offline",
        "already correct",
        "verifies each save",
        "unattempted",
    ] {
        assert!(text.contains(phrase), "missing {phrase}");
    }
}
