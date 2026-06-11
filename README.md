# otter-ai-cli

Unofficial CLI for [otter.ai](http://otter.ai), written in Rust.

> Originally a Python project (forked from [gmchad/otterai-api](https://github.com/gmchad/otterai-api)), rewritten in Rust at full parity. The final Python implementation is preserved at the [`python-final`](https://github.com/andrewfurman/otter-ai-cli/releases/tag/python-final) tag.

## Repository layout

| Path | What it is |
| --- | --- |
| `src/` | All the source. `main.rs` + the per-command modules are the CLI; `client.rs` is the API client and documents every known Otter.ai endpoint; `config.rs` handles credentials. |
| `tests/live.rs` | Integration tests against the real Otter.ai API; they skip themselves unless `OTTERAI_USERNAME`/`OTTERAI_PASSWORD` are set. |
| `Cargo.toml` | The Rust package manifest — name, version, and dependency list. Rust's equivalent of `package.json` / `pyproject.toml`. Edited by hand. |
| `Cargo.lock` | The exact resolved version of every dependency (and their dependencies), so every build uses identical code. Equivalent of `package-lock.json` / `uv.lock`. Managed by cargo — never edited by hand, committed on purpose. |
| `.github/workflows/ci.yml` | Continuous integration: on every push/PR, GitHub runs format check, lint, and tests on a fresh VM. (The `.github/workflows/` folder name is mandated by GitHub Actions.) |
| `AGENTS.md` | Context file for AI coding agents (the [AGENTS.md standard](https://agents.md)): build/test commands and the non-obvious gotchas. |
| `LICENSE` | MIT, inherited from the upstream Python project this descends from ([gmchad/otterai-api](https://github.com/gmchad/otterai-api)) — MIT requires the notice to stay in derivative work. Note GitHub has **no** default license: without this file the code would be all-rights-reserved. |

## Contents

-   [Installation](#installation)
-   [CLI](#cli)
-   [Library](#library)

## Installation

With a [Rust toolchain](https://rustup.rs) installed, from a clone of this repo:

```bash
cargo install --path .
```

or build without installing (binary at `target/release/otter`):

```bash
cargo build --release
```

## CLI

### Authentication

```bash
# Login (saves credentials to ~/.otterai/config.json)
otter login

# Logout (clears saved credentials)
otter logout

# View current user
otter user
```

You can also set credentials via environment variables (these take precedence over the config file):

```bash
export OTTERAI_USERNAME="your-email@example.com"
export OTTERAI_PASSWORD="your-password"
```

### Important: Speech IDs (otid vs speech_id)

Otter.ai speeches have two identifiers:
- **`speech_id`** (e.g. `22WB27HAEBEJYFCA`) — internal ID, does **NOT** work with API endpoints
- **`otid`** (e.g. `jqb7OHo6mrHtCuMkyLN0nUS8mxY`) — the ID used in all API calls

All CLI commands that accept a `SPEECH_ID` argument expect the **otid** value. Use `otter speeches list` to find otids, or `otter speeches list --json | jq '.speeches[].otid'` for just the IDs.

### Speeches

```bash
# List all speeches
otter speeches list

# List with options
otter speeches list --page-size 10 --source owned

# List speeches from the last N days
otter speeches list --days 2

# List speeches in a specific folder (by name or ID)
otter speeches list --folder "CoverNode"

# Get a specific speech
otter speeches get SPEECH_ID

# Search within a speech
otter speeches search "search query" SPEECH_ID

# Download a speech (formats: txt, pdf, mp3, docx, srt)
otter speeches download SPEECH_ID --format txt

# Upload an audio file
otter speeches upload recording.mp4

# Move to trash
otter speeches trash SPEECH_ID

# Rename a speech
otter speeches rename SPEECH_ID "New Title"

# Move speeches to a folder (by name or ID)
otter speeches move SPEECH_ID --folder "CoverNode"
otter speeches move ID1 ID2 ID3 --folder "CoverNode"

# Move to a new folder (auto-create if it doesn't exist)
otter speeches move SPEECH_ID --folder "New Folder" --create
```

### Speakers

```bash
# List all speakers
otter speakers list

# Create a new speaker
otter speakers create "Speaker Name"

# Tag a speaker on transcript segments
otter speakers tag SPEECH_ID SPEAKER_ID            # list segments
otter speakers tag SPEECH_ID SPEAKER_ID -t UUID    # tag one segment
otter speakers tag SPEECH_ID SPEAKER_ID --all      # tag all segments
```

### Folders and Groups

```bash
# List folders
otter folders list

# Create a folder
otter folders create "My Folder"

# Rename a folder
otter folders rename FOLDER_ID "New Name"

# List groups
otter groups list
```

### Configuration

```bash
# Show current config
otter config show

# Clear saved config
otter config clear
```

### JSON Output

Most commands support `--json` flag for machine-readable output:

```bash
otter speeches list --json
otter speakers list --json
```

## Library

The API client lives in `src/client.rs`. Every method mirrors an Otter.ai
endpoint and returns an `ApiResponse { status, data }`, where `data` is the raw
JSON — the API is unofficial and drifts, so the client stays schema-light on
purpose. If you ever need to build an integration in another language, this
file is the endpoint reference.

```rust
use otter::Client;

let mut client = Client::new()?;
client.login("USERNAME", "PASSWORD")?;
let speeches = client.get_speeches("0", 45, "owned")?;
for speech in speeches.data["speeches"].as_array().unwrap_or(&vec![]) {
    println!("{}", speech["title"]);
}
```

Live API tests are gated on `OTTERAI_USERNAME`/`OTTERAI_PASSWORD` being set:

```bash
cargo test
```
