use std::io::{self, BufRead, IsTerminal, Write};

use otter::{config, Client};

use crate::util::{api, die, fail, print_json, result_repr};

/// Mirror of the Python CLI's get_authenticated_client().
pub fn authenticated_client() -> Client {
    let (username, password) = config::load_credentials();
    let (Some(username), Some(password)) = (username, password) else {
        die("Not logged in. Run 'otter login' first.");
    };

    let mut client = match Client::new() {
        Ok(client) => client,
        Err(err) => die(format!("Login failed: {err}")),
    };
    let result = api(client.login(&username, &password));
    if !result.ok() {
        die(format!("Login failed: {}", result_repr(&result)));
    }
    client
}

pub fn login(username: Option<String>, password: Option<String>) {
    let username = username.unwrap_or_else(|| prompt("Username: "));
    let password = password.unwrap_or_else(prompt_password);

    let mut client = match Client::new() {
        Ok(client) => client,
        Err(err) => fail(format!("Login failed: {err}")),
    };
    let result = api(client.login(&username, &password));
    if !result.ok() {
        fail(format!("Login failed: {}", result.data));
    }

    if let Err(err) = config::save_credentials(&username, &password) {
        fail(format!("Could not save credentials: {err}"));
    }
    let email = match &result.data["email"] {
        serde_json::Value::String(s) => s.clone(),
        _ => username,
    };
    println!("Logged in as {email}");
    println!("Credentials saved to {}", config::config_path().display());
}

pub fn logout() {
    match config::clear_credentials() {
        Ok(true) => println!("Credentials cleared."),
        Ok(false) => println!("No saved credentials found."),
        Err(err) => fail(format!("Could not clear credentials: {err}")),
    }
}

pub fn user() {
    let client = authenticated_client();
    let result = api(client.get_user());
    if !result.ok() {
        fail(format!("Failed to get user: {}", result_repr(&result)));
    }
    print_json(&result.data);
}

pub fn config_show() {
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
                Some(password) => println!("Password: {}", "*".repeat(password.chars().count())),
                None => println!("Password: Not set"),
            }
        }
        None => println!("Not logged in."),
    }
}

pub fn config_clear() {
    match config::clear_credentials() {
        Ok(true) => println!("Configuration cleared."),
        Ok(false) => println!("No configuration found."),
        Err(err) => fail(format!("Could not clear configuration: {err}")),
    }
}

pub fn prompt(label: &str) -> String {
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
