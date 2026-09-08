mod auth;
mod folders;
mod groups;
mod pagination;
mod speakers;
mod speeches;
mod util;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

const RATE_LIMIT_HELP: &str = "Rate limits (observed, not an official quota):
  Separate authenticated commands each log in again. Bursts can receive HTTP 429
  from /login before the requested operation runs. Batch selected speaker tags
  with repeated -t UUID flags or -t UUID1,UUID2; batch folder moves with multiple IDs.
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
}
