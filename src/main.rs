mod ask;
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

const RATE_LIMIT_NOTE: &str = "Rate limits: rapid logins can get HTTP 429 from Otter (not an official quota). Stop, wait 60-90 seconds, retry slowly; see README.";

/// Command groups shown in the top-level help. Every command must appear in
/// exactly one group (enforced by a test).
const HELP_GROUPS: &[(&str, &[&str])] = &[
    (
        "Sign in & settings",
        &["login", "logout", "user", "config show", "config clear"],
    ),
    (
        "Read (never changes anything)",
        &[
            "search",
            "ask",
            "speeches list",
            "speeches get",
            "speeches search",
            "speeches download",
            "speakers list",
            "folders list",
            "groups list",
        ],
    ),
    (
        "Change (see each command's --help first)",
        &[
            "speeches rename",
            "speeches rename-batch",
            "speeches upload",
            "speeches move",
            "speeches trash",
            "speakers create",
            "speakers tag",
            "speakers untag",
            "folders create",
            "folders rename",
        ],
    ),
];

const ROOT_TOP: &str = "First time: run `otter login` (asks for your Otter.ai email and password).
Credentials are saved in ~/.otterai/config.json; OTTERAI_USERNAME / OTTERAI_PASSWORD override it.
Speech IDs are OTIDs (the `otid` shown by `otter speeches list`), not numeric ids.";

const ROOT_EXAMPLES: &str = "Examples:
  otter login
  otter speeches list --days 7
  otter search \"budget review\" --from 2026-01-01 --to 2026-01-31
  otter search --speaker \"Jane Doe\" --days 30 --json
  otter speeches get a1B2c3D4e5F6g7H8
  otter speeches download a1B2c3D4e5F6g7H8 --format txt,srt --output meeting.zip
  otter speeches rename-batch plan.json --dry-run
  otter ask \"What did we decide about the Fabrikam contract?\"";

const ROOT_FOOTER: &str =
    "More help: `otter help <command>` or `otter <command> --help` (e.g. otter help speeches list).
Output: readable text; most read commands accept --json. Help never contacts Otter.
Safety: reads change nothing. Commands that change data say so in their help;
trash asks to confirm (-y skips) and speakers untag --all needs --yes.";

const TAG_HELP: &str = "Examples:
  otter speakers list
  otter speakers tag a1B2c3D4e5F6g7H8 42
  otter speakers tag a1B2c3D4e5F6g7H8 42 -t UUID1 -t UUID2
  otter speakers tag a1B2c3D4e5F6g7H8 42 -t UUID1,UUID2 --json
  otter speakers tag a1B2c3D4e5F6g7H8 42 --all

Without -t or --all this only lists segments. Selected segments share one login and
one HTTP session. Duplicate UUIDs are removed, and every UUID is checked against this
conversation before any tags save.
--all assigns the chosen speaker to EVERY segment, including other people's turns.
On HTTP 429, stop and honor Retry-After / retry_after; without a delay, wait 60-90
seconds and retry slowly. The batch stops on its first error, reports progress,
and exits nonzero. Reload a failed segment before retrying an uncertain write.";

const UNTAG_HELP: &str = "Examples:
  otter speakers untag a1B2c3D4e5F6g7H8
  otter speakers untag a1B2c3D4e5F6g7H8 -t UUID1 -t UUID2
  otter speakers untag a1B2c3D4e5F6g7H8 -t UUID1,UUID2 --json
  otter speakers untag a1B2c3D4e5F6g7H8 --all --yes

Without -t or --all this only lists segments. Selected segments share one login and
one HTTP session. Duplicate UUIDs are removed, and every UUID is checked against this
conversation before any clears save.
--all removes the speaker tag from EVERY segment and requires --yes to confirm.
On HTTP 429, stop and honor Retry-After / retry_after; without a delay, wait 60-90
seconds and retry slowly. The batch stops on its first error, reports progress,
and exits nonzero. Reload a failed segment before retrying an uncertain write.";

#[derive(Parser)]
#[command(
    name = "otter",
    version = env!("CARGO_PKG_VERSION"),
    about = "otter - command-line client for your Otter.ai conversations (unofficial).",
    help_template = "{about}\n\nUsage: {usage}\n\n{before-help}Options:\n{options}\n"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ask Otter's AI Chat a question across all your conversations
    #[command(after_help = "Examples:
  otter ask \"What action items came out of last week's meetings?\"
  otter ask \"Summarize the Fabrikam kickoff\" --json
  otter ask \"List every deadline mentioned this month\" --timeout 300

Read-only. The answer is printed as text, or as JSON with --json. Answers can take
a while; raise --timeout if it gives up. If it cannot find a chat token, retry with
--debug (prints only where tokens were looked for, never the secrets).")]
    Ask {
        /// Your question, in quotes (plain English)
        question: String,
        /// Print the answer as JSON instead of text
        #[arg(long)]
        json: bool,
        /// Give up after this many seconds (whole number, 1 or more)
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
        /// Show how the chat token was found (source and key names only, no secrets)
        #[arg(long)]
        debug: bool,
    },
    /// Sign in to Otter.ai and save your credentials
    #[command(after_help = "Examples:
  otter login
  otter login --username jane.doe@example.com
  OTTERAI_USERNAME=jane.doe@example.com OTTERAI_PASSWORD=... otter user

Credentials are saved in plain text in ~/.otterai/config.json (readable only by you).
Prefer the prompt over --password: command-line values can end up in shell history.
Undo with `otter logout`.")]
    Login {
        /// Otter.ai account email; prompted for if omitted
        #[arg(short, long)]
        username: Option<String>,
        /// Otter.ai password; prompted for (hidden) if omitted. Visible in shell history if given here
        #[arg(short, long)]
        password: Option<String>,
    },
    /// Delete the saved credentials (same as `config clear`)
    #[command(after_help = "Examples:
  otter logout

Removes ~/.otterai/config.json from this computer. Nothing changes in your Otter.ai
account. Run `otter login` to sign in again.")]
    Logout,
    /// Show the signed-in Otter.ai account (a quick check that login works)
    #[command(after_help = "Examples:
  otter user

Read-only. Prints your account details as JSON. Fails with 'Not logged in' until
you run `otter login`.")]
    User,
    /// Work with conversations (list, read, download, rename, upload, move, trash)
    #[command(
        subcommand,
        after_help = "Examples:
  otter speeches list --days 7
  otter speeches get a1B2c3D4e5F6g7H8
  otter speeches download a1B2c3D4e5F6g7H8 --format txt
  otter speeches move a1B2c3D4e5F6g7H8 --folder \"Client calls\"

Use `otter help speeches <command>` for each command's flags and examples."
    )]
    Speeches(SpeechesCommand),
    /// Work with speaker names (list, create, tag or untag transcript segments)
    #[command(
        subcommand,
        after_help = "Examples:
  otter speakers list
  otter speakers create \"Jane Doe\"
  otter speakers tag a1B2c3D4e5F6g7H8 42 -t UUID1,UUID2

Use `otter help speakers <command>` for each command's flags and examples."
    )]
    Speakers(SpeakersCommand),
    /// Work with folders (list, create, rename)
    #[command(
        subcommand,
        after_help = "Examples:
  otter folders list
  otter folders create \"Client calls\"
  otter folders rename 1234567 \"Client calls 2026\"

Use `otter help folders <command>` for each command's flags and examples."
    )]
    Folders(FoldersCommand),
    /// Work with groups (list)
    #[command(
        subcommand,
        after_help = "Examples:
  otter groups list
  otter groups list --json"
    )]
    Groups(GroupsCommand),
    /// Show or clear the saved configuration
    #[command(
        subcommand,
        after_help = "Examples:
  otter config show
  otter config clear

The configuration is ~/.otterai/config.json (username and password)."
    )]
    Config(ConfigCommand),
    /// Search all your conversations by keyword, speaker and/or date range
    #[command(after_help = "Examples:
  otter search \"budget review\"
  otter search \"budget\" --days 30 --sort recent
  otter search --speaker \"Jane Doe\" --from 2026-01-01 --to 2026-01-31
  otter search roadmap --speaker \"Jane Doe\" --speaker \"John Smith\" --json
  otter search --speaker \"Jane Doe\" --all --max-seconds 300

Read-only. QUERY is optional if you give --speaker or a date range. With several
--speaker values only conversations containing ALL of them match. Dates are
YYYY-MM-DD in America/New_York; --to is inclusive and needs --from. --days cannot be
combined with --from/--to. Otter's search is nondeterministic, so --tries runs it
several times and merges the results.")]
    Search {
        /// Word or phrase to look for (optional if filtering by --speaker or dates)
        query: Option<String>,
        /// Only conversations with this speaker's display name; repeat or comma-separate for several (all must match)
        #[arg(long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        speaker: Vec<String>,
        /// Search the full archive for --speaker matches (slow; limited by --max-seconds)
        #[arg(long)]
        all: bool,
        /// Print request and response debug info to stderr
        #[arg(long)]
        debug: bool,
        /// How many times to repeat the search and merge results, 1-10
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=10))]
        tries: u32,
        /// Time budget in seconds for multi-window or speaker searches, 10-600
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(10..=600))]
        max_seconds: u32,
        /// First day to include, YYYY-MM-DD (America/New_York), e.g. 2026-01-01
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        from: Option<String>,
        /// Last day to include, YYYY-MM-DD (inclusive); requires --from
        #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new(), requires = "from")]
        to: Option<String>,
        /// Only the last N calendar days, whole number 1 or more; cannot be used with --from/--to
        #[arg(long, conflicts_with_all = ["from", "to"], value_parser = clap::value_parser!(u32).range(1..))]
        days: Option<u32>,
        /// Result order: relevant (best matches first) or recent (newest first)
        #[arg(long, default_value = "relevant", value_parser = ["recent", "relevant"])]
        sort: String,
        /// Maximum number of results, whole number 1 or more (default: 500)
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
        /// Print results as JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SpeechesCommand {
    /// List conversations (newest first) with their OTIDs
    #[command(after_help = "Examples:
  otter speeches list
  otter speeches list --days 7
  otter speeches list --all --json
  otter speeches list --folder \"Client calls\" --source all
  otter speeches list --days 30 --speaker \"Jane Doe\"

Read-only. Without --days or --all you only get one page (--page-size). --days
fetches the whole window by creation time, then applies --speaker. --max-pages is a
safety stop: you get partial results and an error if it is reached.")]
    List {
        /// Folder ID or folder name to list (0 means all folders)
        #[arg(short, long, default_value = "0")]
        folder: String,
        /// Conversations requested per page, whole number 1 or more (Otter may cap it)
        #[arg(short = 'n', long, default_value_t = 45, value_parser = clap::value_parser!(u32).range(1..))]
        page_size: u32,
        /// Whose conversations: owned, shared (shared with you), or all
        #[arg(short, long, default_value = "owned", value_parser = ["owned", "shared", "all"])]
        source: String,
        /// Fetch every conversation created in the last N days, whole number 1 or more
        #[arg(short, long, value_parser = clap::value_parser!(i64).range(1..))]
        days: Option<i64>,
        /// Fetch every page (or the whole --days window) using one login
        #[arg(long)]
        all: bool,
        /// Stop with partial results and an error after this many pages
        #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..))]
        max_pages: u32,
        /// Only conversations with this speaker (name substring, case-insensitive, or speaker id)
        #[arg(long)]
        speaker: Option<String>,
        /// Print the listing as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show one conversation's details and transcript
    #[command(after_help = "Examples:
  otter speeches get a1B2c3D4e5F6g7H8
  otter speeches get a1B2c3D4e5F6g7H8 --json

Read-only. Find OTIDs with `otter speeches list`. --json prints Otter's raw
response, including every transcript segment and its UUID (used by speakers tag).")]
    Get {
        /// Conversation OTID from `otter speeches list` (not the numeric speech_id)
        speech_id: String,
        /// Print Otter's raw response as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Search for a word or phrase inside one conversation's transcript
    #[command(after_help = "Examples:
  otter speeches search \"pricing\" a1B2c3D4e5F6g7H8
  otter speeches search \"deadline\" a1B2c3D4e5F6g7H8 --speaker \"Jane Doe\" -n 20
  otter speeches search \"pricing\" a1B2c3D4e5F6g7H8 --json

Read-only. The QUERY comes first, then the OTID. To search every conversation use
`otter search` instead.")]
    Search {
        /// Word or phrase to look for (quote it if it has spaces)
        query: String,
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// Maximum number of matches, whole number
        #[arg(short = 'n', long, default_value_t = 500)]
        size: u32,
        /// Only matches spoken by this speaker (name substring, case-insensitive, or speaker id)
        #[arg(long)]
        speaker: Option<String>,
        /// Print matches as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Change one conversation's title
    #[command(after_help = "Examples:
  otter speeches rename a1B2c3D4e5F6g7H8 \"2026-01-15 Fabrikam kickoff\"
  otter speeches rename a1B2c3D4e5F6g7H8 \"Weekly sync\"

Changes the title in your Otter.ai account immediately (no confirmation; rename
again to undo). To retitle many conversations safely use `speeches rename-batch`.")]
    Rename {
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// New title (quote it if it has spaces)
        title: String,
    },
    /// Rename many conversations from a JSON plan file (preview first with --dry-run)
    #[command(after_help = "Examples:
  otter speeches rename-batch plan.json --dry-run
  otter speeches rename-batch plan.json
  otter speeches rename-batch plan.json --dry-run --json

Plan file: a JSON array of {\"otid\": ..., \"old_title\": ..., \"new_title\": ...}, e.g.
  [{\"otid\": \"a1B2c3D4e5F6g7H8\", \"old_title\": \"Untitled\", \"new_title\": \"Fabrikam kickoff\"}]
old_title is a string, or null if the conversation has no title.
--dry-run works offline (no login). Applying checks each current title first, skips
titles already correct, and verifies each save. The first error stops the batch and
reports saved, unchanged, failed, and unattempted OTIDs. Reload an unconfirmed
write before retrying. See README for a complete example.")]
    RenameBatch {
        /// Path to the JSON plan file
        file: std::path::PathBuf,
        /// Validate and preview without logging in or changing anything
        #[arg(long)]
        dry_run: bool,
        /// Print the preview or final report as JSON (progress goes to stderr)
        #[arg(long)]
        json: bool,
    },
    /// Download a conversation as text, PDF, audio, Word or subtitles
    #[command(after_help = "Examples:
  otter speeches download a1B2c3D4e5F6g7H8
  otter speeches download a1B2c3D4e5F6g7H8 --format pdf --output notes.pdf
  otter speeches download a1B2c3D4e5F6g7H8 -f txt,srt,mp3
  otter speeches download a1B2c3D4e5F6g7H8 -f docx -o meeting.docx

Writes a file in the current directory (or --output) and overwrites an existing file
of the same name. Formats: txt, pdf, mp3, docx, srt.")]
    Download {
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// Format: txt, pdf, mp3, docx or srt; comma-separate several to get a zip
        #[arg(short, long, default_value = "txt")]
        format: String,
        /// Exact output path; default: OTID.<format>, or OTID.zip for multiple formats
        #[arg(short, long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        output: Option<String>,
    },
    /// Upload an audio file to be transcribed
    #[command(after_help = "Examples:
  otter speeches upload meeting.m4a
  otter speeches upload call.mp3 --content-type audio/mpeg
  otter speeches upload recording.wav -t audio/wav

Creates a new conversation in your Otter.ai account. Transcription runs in the background; check `otter speeches list` later.
The default type audio/mp4 suits .m4a and .mp4 files; set --content-type for others.")]
    Upload {
        /// Path to the audio file on this computer
        file: String,
        /// MIME type of the file, e.g. audio/mpeg, audio/wav
        #[arg(short = 't', long, default_value = "audio/mp4")]
        content_type: String,
    },
    /// Move one conversation to the trash
    #[command(after_help = "Examples:
  otter speeches trash a1B2c3D4e5F6g7H8
  otter speeches trash a1B2c3D4e5F6g7H8 --yes

Moves the conversation to Otter's trash (it leaves your normal lists). Without
--yes you must type y to continue.")]
    Trash {
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// Do not ask for confirmation (use in scripts)
        #[arg(short, long)]
        yes: bool,
    },
    /// Move one or more conversations into a folder
    #[command(after_help = "Examples:
  otter speeches move a1B2c3D4e5F6g7H8 --folder \"Client calls\"
  otter speeches move OTID1 OTID2 OTID3 --folder 1234567
  otter speeches move a1B2c3D4e5F6g7H8 --folder \"Q1 reviews\" --create

Pass OTIDs as separate arguments. Duplicates are removed. Success requires every
OTID in Otter's acknowledgement; partial or malformed results exit nonzero and
report unconfirmed completion. Reload affected recordings before retrying.
Without --create a missing folder is an error. Find folders with `otter folders list`.")]
    Move {
        /// One or more conversation OTIDs, separated by spaces
        #[arg(required = true)]
        speech_ids: Vec<String>,
        /// Destination folder ID or folder name
        #[arg(short, long)]
        folder: String,
        /// Create the folder if no folder has that name (only after a successful lookup confirms it is missing)
        #[arg(long)]
        create: bool,
    },
}

#[derive(Subcommand)]
enum SpeakersCommand {
    /// List known speakers and their ids
    #[command(after_help = "Examples:
  otter speakers list
  otter speakers list --json

Read-only. Use the id column as SPEAKER_ID in `otter speakers tag`.")]
    List {
        /// Print speakers as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Create a new named speaker
    #[command(after_help = "Examples:
  otter speakers create \"Jane Doe\"
  otter speakers create \"John Smith\"

Adds a speaker name to your account (nothing is tagged yet). Use `otter speakers
list` to get its id, then `otter speakers tag`.")]
    Create {
        /// Speaker's display name (quote it if it has spaces)
        name: String,
    },
    /// Assign a speaker to transcript segments (lists segments if none chosen)
    #[command(after_help = TAG_HELP)]
    Tag {
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// Speaker id from `otter speakers list`
        speaker_id: String,
        /// Segment UUID(s) to tag; repeat -t or separate with commas (UUIDs come from `speeches get --json`)
        #[arg(short, long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        transcript_uuid: Vec<String>,
        /// Assign this speaker to EVERY segment in the conversation (cannot be used with -t)
        #[arg(short, long, conflicts_with = "transcript_uuid")]
        all: bool,
        /// Print the segment listing or batch result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove the speaker from transcript segments (lists segments if none chosen)
    #[command(after_help = UNTAG_HELP)]
    Untag {
        /// Conversation OTID from `otter speeches list`
        speech_id: String,
        /// Segment UUID(s) to clear; repeat -t or separate with commas (UUIDs come from `speeches get --json`)
        #[arg(short, long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
        transcript_uuid: Vec<String>,
        /// Clear the speaker from EVERY segment in the conversation (dangerous; needs --yes; cannot be used with -t)
        #[arg(short, long, conflicts_with = "transcript_uuid")]
        all: bool,
        /// Confirm a destructive --all
        #[arg(long)]
        yes: bool,
        /// Print the segment listing or batch result as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FoldersCommand {
    /// List your folders and their ids
    #[command(after_help = "Examples:
  otter folders list
  otter folders list --json

Read-only. Folder ids and names both work wherever a FOLDER is asked for.")]
    List {
        /// Print folders as JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Create a new folder
    #[command(after_help = "Examples:
  otter folders create \"Client calls\"
  otter folders create \"Q1 reviews\" --json

Creates a folder in your Otter.ai account (rename it later with `folders rename`).
Check `otter folders list` first so you do not create a second folder with the same name.")]
    Create {
        /// Folder name (quote it if it has spaces)
        name: String,
        /// Print the created folder as JSON
        #[arg(long)]
        json: bool,
    },
    /// Rename a folder
    #[command(after_help = "Examples:
  otter folders rename 1234567 \"Client calls 2026\"

Changes the folder name in your Otter.ai account immediately; rename it back to
undo. Get FOLDER_ID from `otter folders list`.")]
    Rename {
        /// Folder id from `otter folders list`
        folder_id: String,
        /// New folder name (quote it if it has spaces)
        new_name: String,
    },
}

#[derive(Subcommand)]
enum GroupsCommand {
    /// List your groups
    #[command(after_help = "Examples:
  otter groups list
  otter groups list --json

Read-only.")]
    List {
        /// Print groups as JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show where credentials are stored (password masked)
    #[command(after_help = "Examples:
  otter config show

Read-only. The password is masked. Environment variables OTTERAI_USERNAME and
OTTERAI_PASSWORD take precedence over the file when both are set.")]
    Show,
    /// Delete the saved configuration (same as `logout`)
    #[command(after_help = "Examples:
  otter config clear

Deletes ~/.otterai/config.json from this computer. Nothing changes in your Otter.ai
account. Run `otter login` to sign in again.")]
    Clear,
}

/// Builds the CLI with a generated, grouped command list in the top-level help.
fn cli_command() -> clap::Command {
    let command = Cli::command();
    let mut about = std::collections::HashMap::new();
    collect_about(&command, "", &mut about);
    let mut body = format!("{ROOT_TOP}\n");
    for (heading, paths) in HELP_GROUPS {
        body.push_str(&format!("\n{heading}:\n"));
        for path in *paths {
            let text = about.get(*path).cloned().unwrap_or_default();
            body.push_str(&format!("  {:<30} {text}\n", format!("otter {path}")));
        }
    }
    let mut ungrouped: Vec<_> = about
        .keys()
        .filter(|k| !HELP_GROUPS.iter().any(|(_, p)| p.contains(&k.as_str())))
        .collect();
    ungrouped.sort();
    if !ungrouped.is_empty() {
        body.push_str("\nOther:\n");
        for path in ungrouped {
            body.push_str(&format!(
                "  {:<30} {}\n",
                format!("otter {path}"),
                about[path]
            ));
        }
    }
    body.push_str(&format!(
        "\n{ROOT_EXAMPLES}\n\n{ROOT_FOOTER}\n{RATE_LIMIT_NOTE}"
    ));
    command.before_help(body)
}

/// Maps each leaf command path ("speeches list") to its one-line description.
fn collect_about(
    command: &clap::Command,
    prefix: &str,
    output: &mut std::collections::HashMap<String, String>,
) {
    for subcommand in command.get_subcommands() {
        let path = if prefix.is_empty() {
            subcommand.get_name().to_string()
        } else {
            format!("{prefix} {}", subcommand.get_name())
        };
        if subcommand.has_subcommands() {
            collect_about(subcommand, &path, output);
        } else {
            let about = subcommand
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            output.insert(path, about);
        }
    }
}

/// Prints a clap error (or help/version) and exits with clap's own exit code,
/// adding a pointer to the help for real mistakes.
fn exit_with_hint(error: clap::Error) -> ! {
    let is_mistake = error.use_stderr();
    let code = error.exit_code();
    let _ = error.print();
    if is_mistake {
        eprintln!(
            "\nHelp: `otter help` lists commands and examples; `otter help <command>` shows a\ncommand's flags and examples (e.g. otter help speeches list)."
        );
    }
    std::process::exit(code)
}

fn main() {
    let matches = cli_command()
        .try_get_matches()
        .unwrap_or_else(|error| exit_with_hint(error));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| exit_with_hint(error));
    match cli.command {
        Command::Ask {
            question,
            json,
            timeout,
            debug,
        } => ask::ask(question, json, timeout, debug),
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

    fn walk(command: &clap::Command, path: &str, problems: &mut Vec<String>) {
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let path = format!("{path} {}", sub.get_name());
            if sub.get_about().is_none() {
                problems.push(format!("{path}: no one-line description"));
            }
            let examples = sub
                .get_after_help()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if !examples.contains("Examples:") || !examples.contains("otter ") {
                problems.push(format!("{path}: no Examples section"));
            }
            for arg in sub.get_arguments() {
                if arg.get_id() == "help" || arg.get_id() == "version" {
                    continue;
                }
                let blank = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
                if blank.trim().is_empty() {
                    problems.push(format!("{path}: argument '{}' has no help", arg.get_id()));
                }
            }
            walk(sub, &path, problems);
        }
    }

    #[test]
    fn every_subcommand_has_examples_and_every_argument_has_help() {
        let mut problems = Vec::new();
        walk(&Cli::command(), "otter", &mut problems);
        assert!(problems.is_empty(), "help gaps:\n{}", problems.join("\n"));
    }

    #[test]
    fn every_command_is_in_exactly_one_top_level_help_group() {
        let mut about = std::collections::HashMap::new();
        collect_about(&Cli::command(), "", &mut about);
        for path in about.keys() {
            let n = HELP_GROUPS
                .iter()
                .filter(|(_, p)| p.contains(&path.as_str()))
                .count();
            assert_eq!(n, 1, "{path} is in {n} help groups");
        }
        for (_, paths) in HELP_GROUPS {
            for p in *paths {
                assert!(about.contains_key(*p), "unknown command in group: {p}");
            }
        }
    }

    #[test]
    fn help_examples_use_placeholder_data_only() {
        let mut text = String::new();
        let mut cmd = cli_command();
        text.push_str(&cmd.render_long_help().to_string());
        fn all(c: &clap::Command, t: &mut String) {
            for s in c.get_subcommands() {
                t.push_str(
                    &s.get_after_help()
                        .map(|x| x.to_string())
                        .unwrap_or_default(),
                );
                all(s, t);
            }
        }
        all(&cmd, &mut text);
        for ok in ["@example.com", "Jane Doe"] {
            assert!(text.contains(ok));
        }
        assert!(!text.contains("furman") && !text.contains("gmail"));
    }

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
