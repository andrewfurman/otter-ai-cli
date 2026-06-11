mod auth;
mod folders;
mod groups;
mod speakers;
mod speeches;
mod util;

use clap::{Parser, Subcommand};

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
    /// List all speeches
    List {
        /// Folder ID or name (default: 0 = all)
        #[arg(short, long, default_value = "0")]
        folder: String,
        /// Number of results (default: 45)
        #[arg(short = 'n', long, default_value_t = 45)]
        page_size: u32,
        /// Source filter (default: owned)
        #[arg(short, long, default_value = "owned", value_parser = ["owned", "shared", "all"])]
        source: String,
        /// Only show speeches from the last N days
        #[arg(short, long)]
        days: Option<i64>,
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
        /// Output filename (optional)
        #[arg(short, long)]
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
    Move {
        #[arg(required = true)]
        speech_ids: Vec<String>,
        /// Destination folder ID or name
        #[arg(short, long)]
        folder: String,
        /// Create the folder if it doesn't exist (when using folder name)
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
    Tag {
        speech_id: String,
        speaker_id: String,
        /// Specific transcript UUID to tag
        #[arg(short, long)]
        transcript_uuid: Option<String>,
        /// Tag all segments with this speaker
        #[arg(short, long)]
        all: bool,
        /// Output as JSON
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

fn main() {
    match Cli::parse().command {
        Command::Login { username, password } => auth::login(username, password),
        Command::Logout => auth::logout(),
        Command::User => auth::user(),
        Command::Speeches(command) => match command {
            SpeechesCommand::List {
                folder,
                page_size,
                source,
                days,
                json,
            } => speeches::list(folder, page_size, source, days, json),
            SpeechesCommand::Get { speech_id, json } => speeches::get(speech_id, json),
            SpeechesCommand::Search {
                query,
                speech_id,
                size,
                json,
            } => speeches::search(query, speech_id, size, json),
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
