mod auth;
mod batch_rename;
mod folders;
mod groups;
mod pagination;
mod search;
mod speakers;
mod speeches;
mod util;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

const RATE_LIMIT_HELP: &str = "Rate limits (observed, not an official quota):
  Separate authenticated commands each log in again. Bursts can receive HTTP 429
  from /login before the requested operation runs. Batch selected speaker tags
  with repeated -t UUID flags or -t UUID1,UUID2; batch folder moves with multiple IDs.
  Use speeches rename-batch for title plans. Listing --days/--all pages share a login.
  On 429, stop and honor Retry-After / retry_after. If no delay is supplied, wait
  60-90 seconds, then retry slowly. Longer pauses may be needed; avoid parallel
  retry loops. A tag batch stops on its first error and reports saved/unattempted IDs.
  No fixed requests-per-minute limit has been established. See README for findings.";

const TAG_HELP: &str = "Examples:
  otter speakers tag OTID SPEAKER_ID              List segments without changing them
  otter speakers tag OTID SPEAKER_ID -t UUID1 -t UUID2
  otter speakers tag OTID SPEAKER_ID -t UUID1,UUID2 --json

Selected segments share one login and one HTTP session. Duplicate UUIDs are
removed, and every UUID is checked against this conversation before any tags save.
--all assigns the chosen speaker to EVERY segment, including other people's turns.
On HTTP 429, stop and honor Retry-After / retry_after; without a delay, wait 60-90
seconds and retry slowly. The batch stops on its first error, reports progress,
and exits nonzero. Reload a failed segment before retrying an uncertain write.";

#[derive(Parser)]
#[command(
    name = "otter",
    version = "0.1.0",
    about = "OtterAI CLI - Interact with Otter.ai from the command line."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with Otter.ai and save credentials
    Login {
        /// Otter.ai username (email)
        #[arg(short, long)]
        username: Option<String>,
        /// Otter.ai password
        #[arg(short, long)]
        password: Option<String>,
    },
    /// Clear saved credentials
    Logout,
    /// Show current user information
    User,
    /// Manage speeches (transcripts)
    #[command(subcommand)]
    Speeches(SpeechesCommand),
    /// Manage speakers
    #[command(subcommand)]
    Speakers(SpeakersCommand),
    /// Manage folders
    #[command(subcommand)]
    Folders(FoldersCommand),
    /// Manage groups
    #[command(subcommand)]
    Groups(GroupsCommand),
    /// Manage CLI configuration
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Search conversations across your archive (keyword/speakers/date window)
    Search {
        /// Optional keyword to search for
        query: Option<String>,
        /// Filter by speaker display name (repeatable)
        #[arg(long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        speaker: Vec<String>,
        /// Search the full archive for speakers (bounded by --max-seconds)
        #[arg(long)]
        all: bool,
        /// Print request/response debug info to stderr
        #[arg(long)]
        debug: bool,
        /// Number of tries for unioning nondeterministic search (1-10, default 5)
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=10))]
        tries: u32,
        /// Overall wall-clock budget in seconds for multi-window/speaker searches (default 120)
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(10..=600))]
        max_seconds: u32,
        /// Start date (YYYY-MM-DD) in America/New_York
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        from: Option<String>,
        /// End date (YYYY-MM-DD), inclusive; sent as next day's midnight ET
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new(), requires = "from")]
        to: Option<String>,
        /// Last N calendar days (conflicts with --from/--to)
        #[arg(long, conflicts_with_all = ["from", "to"], value_parser = clap::value_parser!(u32).range(1..))]
        days: Option<u32>,
        /// Sort results by relevance (default) or most recent
        #[arg(long, default_value = "relevant", value_parser = ["recent", "relevant"])]
        sort: String,
        /// Max results to return (default: 500)
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SpeechesCommand {
    /// List speeches; --days paginates its date window, --all fetches every page
    List {
        /// Folder ID or name (default: 0 = all)
        #[arg(short, long, default_value = "0")]
        folder: String,
        /// Requested conversations per page before filtering (the server may cap this)
        #[arg(short = 'n', long, default_value_t = 45, value_parser = clap::value_parser!(u32).range(1..))]
        page_size: u32,
        /// Source filter (default: owned)
        #[arg(short, long, default_value = "owned", value_parser = ["owned", "shared", "all"])]
        source: String,
        /// Fetch the complete last N days by creation time, then apply speaker filtering
        #[arg(short, long, value_parser = clap::value_parser!(i64).range(1..))]
        days: Option<i64>,
        /// Fetch every page (or the complete --days window) with one login
        #[arg(long)]
        all: bool,
        /// Stop with partial results and an error after this many pages
        #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..))]
        max_pages: u32,
        /// Filter by speaker name (case-insensitive substring) or speaker id
        #[arg(long)]
        speaker: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Get details of a specific speech
    Get {
        speech_id: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Search within a speech transcript
    Search {
        query: String,
        speech_id: String,
        /// Max results (default: 500)
        #[arg(short = 'n', long, default_value_t = 500)]
        size: u32,
        /// Filter by speaker name (case-insensitive substring) or speaker id
        #[arg(long)]
        speaker: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Rename a speech (set new title)
    Rename { speech_id: String, title: String },
    /// Rename recordings from a JSON plan in one authenticated session
    #[command(
        after_help = "Plan: an array of {otid, old_title, new_title}; old_title is a string or null.\nUse --dry-run to validate and preview offline. Apply checks the current title, skips already-correct titles, and verifies each save. The first error stops the batch and reports saved, unchanged, failed, and unattempted OTIDs. Reload an unconfirmed write before retrying. See README for a complete example."
    )]
    RenameBatch {
        /// JSON plan file
        file: std::path::PathBuf,
        /// Validate and preview without logging in or changing recordings
        #[arg(long)]
        dry_run: bool,
        /// Output the preview or completion report as JSON (progress goes to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Download a speech in specified format(s)
    Download {
        speech_id: String,
        /// Format(s): txt, pdf, mp3, docx, srt (comma-separated, default: txt)
        #[arg(short, long, default_value = "txt")]
        format: String,
        /// Exact output path; default: OTID.<format>, or OTID.zip for multiple formats
        #[arg(short, long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        output: Option<String>,
    },
    /// Upload an audio file for transcription
    Upload {
        file: String,
        /// MIME type (default: audio/mp4)
        #[arg(short = 't', long, default_value = "audio/mp4")]
        content_type: String,
    },
    /// Move a speech to trash
    Trash {
        speech_id: String,
        /// Skip confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Move speech(es) to a folder
    #[command(
        after_help = "Pass OTIDs as separate arguments. Duplicates are removed. Success requires every OTID in Otter's acknowledgement; partial or malformed results exit nonzero and report unconfirmed completion. Reload affected recordings before retrying."
    )]
    Move {
        #[arg(required = true)]
        speech_ids: Vec<String>,
        /// Destination folder ID or name
        #[arg(short, long)]
        folder: String,
        /// Create a folder only after a successful lookup confirms the name is missing
        #[arg(long)]
        create: bool,
    },
}

#[derive(Subcommand)]
enum SpeakersCommand {
    /// List all speakers
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Create a new speaker
    Create { name: String },
    /// Tag a speaker on transcript segment(s)
    #[command(after_help = TAG_HELP)]
    Tag {
        speech_id: String,
        speaker_id: String,
        /// Transcript UUID(s) to tag; repeat -t or separate UUIDs with commas
        #[arg(short, long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        transcript_uuid: Vec<String>,
        /// Assign this speaker to EVERY segment in the conversation
        #[arg(short, long, conflicts_with = "transcript_uuid")]
        all: bool,
        /// Output segment listings or batch results as JSON
        #[arg(long)]
        json: bool,
    },
    /// Clear a speaker tag on transcript segment(s)
    #[command(
        after_help = "Examples:\n  otter speakers untag OTID                 List segments without changing them\n  otter speakers untag OTID -t UUID1 -t UUID2\n\nSelected segments share one login and one HTTP session. Duplicate UUIDs are\nremoved, and every UUID is checked against this conversation before any clears save.\n--all removes the speaker tag from EVERY segment; combine with --yes to confirm.\nOn HTTP 429, stop and honor Retry-After / retry_after; without a delay, wait 60-90\nseconds and retry slowly. The batch stops on its first error, reports progress,\nand exits nonzero. Reload a failed segment before retrying an uncertain write."
    )]
    Untag {
        speech_id: String,
        /// Transcript UUID(s) to untag; repeat -t or separate UUIDs with commas
        #[arg(short, long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        transcript_uuid: Vec<String>,
        /// Remove the tag from EVERY segment in the conversation (dangerous)
        #[arg(short, long, conflicts_with = "transcript_uuid")]
        all: bool,
        /// Require explicit confirmation when using --all
        #[arg(long)]
        yes: bool,
        /// Output segment listings or batch results as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FoldersCommand {
    /// List all folders
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Create a new folder
    Create {
        name: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Rename a folder
    Rename { folder_id: String, new_name: String },
}

#[derive(Subcommand)]
enum GroupsCommand {
    /// List all groups
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Clear saved configuration
    Clear,
}

fn cli_command() -> clap::Command {
    let command = Cli::command();
    let mut inventory =
        String::from("All commands (use any command's --help for its arguments):\n");
    command_inventory(&command, "otter", &mut inventory);
    inventory.push_str("  otter help [COMMAND]  Show help for a command or group\n\n");
    inventory.push_str(RATE_LIMIT_HELP);
    command.after_help(inventory)
}

fn command_inventory(command: &clap::Command, prefix: &str, output: &mut String) {
    for subcommand in command.get_subcommands() {
        let path = format!("{prefix} {}", subcommand.get_name());
        if subcommand.has_subcommands() {
            command_inventory(subcommand, &path, output);
        } else {
            let about = subcommand
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            output.push_str(&format!("  {path:<26} {about}\n"));
        }
    }
}

fn main() {
    let matches = cli_command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    match cli.command {
        Command::Login { username, password } => auth::login(username, password),
        Command::Logout => auth::logout(),
        Command::User => auth::user(),
        Command::Speeches(command) => match command {
            SpeechesCommand::List {
                folder,
                page_size,
                source,
                days,
                all,
                max_pages,
                speaker,
                json,
            } => speeches::list(
                folder, page_size, source, days, all, max_pages, speaker, json,
            ),
            SpeechesCommand::Get { speech_id, json } => speeches::get(speech_id, json),
            SpeechesCommand::Search {
                query,
                speech_id,
                size,
                speaker,
                json,
            } => speeches::search(query, speech_id, size, speaker, json),
            SpeechesCommand::Rename { speech_id, title } => speeches::rename(speech_id, title),
            SpeechesCommand::RenameBatch {
                file,
                dry_run,
                json,
            } => batch_rename::run(file, dry_run, json),
            SpeechesCommand::Download {
                speech_id,
                format,
                output,
            } => speeches::download(speech_id, format, output),
            SpeechesCommand::Upload { file, content_type } => speeches::upload(file, content_type),
            SpeechesCommand::Trash { speech_id, yes } => speeches::trash(speech_id, yes),
            SpeechesCommand::Move {
                speech_ids,
                folder,
                create,
            } => speeches::move_to_folder(speech_ids, folder, create),
        },
        Command::Speakers(command) => match command {
            SpeakersCommand::List { json } => speakers::list(json),
            SpeakersCommand::Create { name } => speakers::create(name),
            SpeakersCommand::Tag {
                speech_id,
                speaker_id,
                transcript_uuid,
                all,
                json,
            } => speakers::tag(speech_id, speaker_id, transcript_uuid, all, json),
            SpeakersCommand::Untag {
                speech_id,
                transcript_uuid,
                all,
                yes,
                json,
            } => speakers::untag(speech_id, transcript_uuid, all, yes, json),
        },
        Command::Folders(command) => match command {
            FoldersCommand::List { json } => folders::list(json),
            FoldersCommand::Create { name, json } => folders::create(name, json),
            FoldersCommand::Rename {
                folder_id,
                new_name,
            } => folders::rename(folder_id, new_name),
        },
        Command::Groups(command) => match command {
            GroupsCommand::List { json } => groups::list(json),
        },
        Command::Config(command) => match command {
            ConfigCommand::Show => auth::config_show(),
            ConfigCommand::Clear => auth::config_clear(),
        },
        Command::Search {
            query,
            speaker,
            all,
            debug,
            tries,
            max_seconds,
            from,
            to,
            days,
            sort,
            limit,
            json,
        } => {
            let mode = if sort.eq_ignore_ascii_case("recent") {
                search::SortMode::Recent
            } else {
                search::SortMode::Relevant
            };
            search::run(search::SearchOptions {
                query,
                speakers: speaker,
                all,
                from,
                to,
                days,
                sort: mode,
                limit,
                as_json: json,
                debug,
                tries,
                max_seconds,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_accepts_repeated_and_comma_separated_uuids() {
        let cli = Cli::try_parse_from([
            "otter", "speakers", "tag", "otid", "42", "-t", "a,b", "-t", "c", "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Speakers(SpeakersCommand::Tag {
                transcript_uuid,
                all,
                json,
                ..
            }) => {
                assert_eq!(transcript_uuid, ["a", "b", "c"]);
                assert!(!all);
                assert!(json);
            }
            _ => panic!("expected speakers tag"),
        }
    }

    #[test]
    fn tag_rejects_all_with_selected_uuids_and_empty_values() {
        assert!(Cli::try_parse_from([
            "otter", "speakers", "tag", "otid", "42", "-t", "a", "--all"
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["otter", "speakers", "tag", "otid", "42", "-t", "a,,b"]).is_err()
        );
    }

    #[test]
    fn untag_accepts_repeated_and_comma_separated_uuids() {
        let cli = Cli::try_parse_from([
            "otter", "speakers", "untag", "otid", "-t", "a,b", "-t", "c", "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Speakers(SpeakersCommand::Untag {
                transcript_uuid,
                all,
                yes,
                json,
                ..
            }) => {
                assert_eq!(transcript_uuid, ["a", "b", "c"]);
                assert!(!all);
                assert!(!yes);
                assert!(json);
            }
            _ => panic!("expected speakers untag"),
        }
    }

    #[test]
    fn untag_rejects_all_with_selected_uuids_and_empty_values() {
        assert!(
            Cli::try_parse_from(["otter", "speakers", "untag", "otid", "--all", "-t", "uuid"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["otter", "speakers", "untag", "otid", "-t", "a,,b"]).is_err());
    }

    #[test]
    fn list_and_search_accept_speaker_flag() {
        let list = Cli::try_parse_from(["otter", "speeches", "list", "--speaker", "Alice"])
            .expect("list --speaker should parse");
        match list.command {
            Command::Speeches(SpeechesCommand::List { speaker, .. }) => {
                assert_eq!(speaker.as_deref(), Some("Alice"));
            }
            _ => panic!("expected speeches list"),
        }

        let search = Cli::try_parse_from([
            "otter",
            "speeches",
            "search",
            "hello",
            "otid123",
            "--speaker",
            "42",
        ])
        .expect("search --speaker should parse");
        match search.command {
            Command::Speeches(SpeechesCommand::Search {
                speaker,
                query,
                speech_id,
                ..
            }) => {
                assert_eq!(query, "hello");
                assert_eq!(speech_id, "otid123");
                assert_eq!(speaker.as_deref(), Some("42"));
            }
            _ => panic!("expected speeches search"),
        }
    }

    #[test]
    fn archive_search_parsing_accepts_speakers_dates_and_sort() {
        let cli = Cli::try_parse_from([
            "otter",
            "search",
            "Disney",
            "--speaker",
            "Kate",
            "--speaker",
            "Emily",
            "--from",
            "2026-09-21",
            "--to",
            "2026-09-24",
            "--sort",
            "recent",
            "--limit",
            "50",
            "--json",
        ])
        .expect("search parse");
        match cli.command {
            Command::Search {
                query,
                speaker,
                from,
                to,
                days,
                sort,
                limit,
                json,
                ..
            } => {
                assert_eq!(query.as_deref(), Some("Disney"));
                assert_eq!(speaker, ["Kate", "Emily"]);
                assert_eq!(from.as_deref(), Some("2026-09-21"));
                assert_eq!(to.as_deref(), Some("2026-09-24"));
                assert!(days.is_none());
                assert_eq!(sort, "recent");
                assert_eq!(limit, Some(50));
                assert!(json);
            }
            _ => panic!("expected archive search"),
        }
    }
}
