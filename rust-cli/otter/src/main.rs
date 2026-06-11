use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use otterai::{config, Client};

#[derive(Parser)]
#[command(name = "otter", version, about = "OtterAI CLI (Rust port)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with Otter.ai and save credentials
    Login {
        #[arg(long)]
        username: Option<String>,
    },
    /// Clear saved credentials
    Logout,
    /// Manage CLI configuration
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Clear saved configuration
    Clear,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Login { username } => login(username),
        Command::Logout => clear("credentials"),
        Command::Config(ConfigCommand::Show) => config_show(),
        Command::Config(ConfigCommand::Clear) => clear("configuration"),
    }
}

fn login(username: Option<String>) -> ExitCode {
    let username = username.unwrap_or_else(|| prompt("Email: "));
    let password = prompt_password();

    let mut client = match Client::new() {
        Ok(client) => client,
        Err(err) => return fail(&format!("Login failed: {err}")),
    };
    let data = match client.login(&username, &password) {
        Ok(data) => data,
        Err(err) => return fail(&format!("Login failed: {err}")),
    };

    if let Err(err) = config::save_credentials(&username, &password) {
        return fail(&format!("Could not save credentials: {err}"));
    }
    println!(
        "Logged in as {}",
        data.email.as_deref().unwrap_or(&username)
    );
    println!("Credentials saved to {}", config::config_path().display());
    ExitCode::SUCCESS
}

fn config_show() -> ExitCode {
    let path = config::config_path();
    println!("Config file: {}", path.display());
    println!(
        "Config exists: {}",
        if path.exists() { "True" } else { "False" }
    );

    let (username, password) = config::load_credentials();
    match username {
        Some(username) => {
            println!("Username: {username}");
            match password {
                Some(password) => println!("Password: {}", "*".repeat(password.len())),
                None => println!("Password: Not set"),
            }
        }
        None => println!("Not logged in."),
    }
    ExitCode::SUCCESS
}

fn clear(what: &str) -> ExitCode {
    match config::clear_credentials() {
        Ok(true) => {
            println!(
                "{} cleared.",
                if what == "credentials" {
                    "Credentials"
                } else {
                    "Configuration"
                }
            );
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("No saved {what} found.");
            ExitCode::SUCCESS
        }
        Err(err) => fail(&format!("Could not clear {what}: {err}")),
    }
}

fn prompt(label: &str) -> String {
    print!("{label}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    line.trim().to_string()
}

fn prompt_password() -> String {
    // Fall back to a plain stdin read when piped, so the command stays scriptable.
    if io::stdin().is_terminal() {
        rpassword::prompt_password("Password: ").unwrap_or_default()
    } else {
        prompt("Password: ")
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
