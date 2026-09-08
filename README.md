# otter-ai-cli

Unofficial command-line tool for [Otter.ai](https://otter.ai). It is not affiliated with, endorsed by, or part of Otter.ai.

For people on an [Otter Pro](https://otter.ai/pricing) plan (or another paid Otter plan) who want a simple way for humans and agents to access their own conversations from the terminal.

Single Rust crate: command modules under `src/` are the CLI, `src/client.rs` is the unofficial HTTP client, and `src/config.rs` stores credentials in `~/.otterai/config.json` (`OTTERAI_USERNAME` / `OTTERAI_PASSWORD` override the file).

## Install

Anyone can install this. You need a [Rust toolchain](https://rustup.rs):

```bash
cargo install --git https://github.com/andrewfurman/otter-ai-cli
```

That puts an `otter` binary on your `PATH`. From a checkout: `cargo install --path .`

## Use

Log in with your own Otter account, then list conversations:

```bash
otter login
otter speeches list --days 2
```

Most read commands take `--json` for scripts and agents; `speakers tag --json` also returns a batch result. `speeches list` and `speeches search` accept `--speaker` to filter by speaker name or id. `otter --help` and `otter help` show every command and rate-limit guidance. Use `otter <group> --help` or `otter help <group> <command>` for arguments and examples.

Speaker-filtered searches, transcript display, and tagging previews resolve segment speaker IDs through the conversation's speaker list, including numeric and string IDs. Names match case-insensitive substrings; IDs match exactly. If no name is available in that list, an embedded segment name is used when present. This lookup uses the fetched conversation and adds no API requests. `speeches get --json` preserves the raw API response.

CLI speech IDs are Otter **otid** values (from `otter speeches list`), not the internal `speech_id`.

Use this only with an Otter account you are allowed to access, and follow [Otter.ai's Terms of Service](https://otter.ai/terms-of-service).

## Command reference

| Command | Purpose |
| --- | --- |
| `otter login` | Authenticate and save credentials; prompts if username/password are omitted |
| `otter logout` | Clear saved credentials |
| `otter user` | Show the current account |
| `otter speeches list` | Fetch one page of conversations; filter by folder, source, days, or speaker |
| `otter speeches get OTID` | Fetch a conversation and its transcript segments |
| `otter speeches search QUERY OTID` | Search a transcript, optionally filtering by speaker |
| `otter speeches rename OTID TITLE` | Change a conversation title |
| `otter speeches download OTID` | Export txt, pdf, mp3, docx, or srt; comma-separated formats produce a zip |
| `otter speeches upload FILE` | Upload audio for transcription |
| `otter speeches trash OTID` | Move a conversation to trash; `--yes` skips confirmation |
| `otter speeches move OTID1 OTID2 --folder FOLDER` | Move multiple conversations in one session; `--create` creates a missing folder |
| `otter speakers list` | List known speaker names and IDs |
| `otter speakers create NAME` | Create a named speaker |
| `otter speakers tag OTID SPEAKER_ID` | List segments, or tag selected segments with `-t` |
| `otter folders list` | List folders |
| `otter folders create NAME` | Create a folder |
| `otter folders rename FOLDER_ID NAME` | Rename a folder |
| `otter groups list` | List groups |
| `otter config show` | Show configuration status with the password masked |
| `otter config clear` | Clear saved configuration |
| `otter help [COMMAND]` | Show help, including nested commands such as `help speakers tag` |

Run any command with `--help` for all its flags. Help does not authenticate or contact Otter. The top-level help inventory is generated from the command definitions so new commands appear automatically.

`speeches list` fetches one page (up to `--page-size`, default 45) and then applies date and speaker filters. If Otter reports more results or omits its completeness flag, the CLI prints a notice to stderr, including how many conversations were fetched. No matches in that page does not establish that no matching conversation exists. Increasing `--page-size` requests a larger window, subject to the server's cap. JSON stdout remains parseable and retains Otter's `end_of_list` field when supplied; automatic pagination is not implemented.

`speeches move --create` creates a folder only when a successful folder lookup confirms that its name is missing. A failed lookup stops the command before any folder creation or moves, including permission, rate-limit, network, and malformed-response errors. Creation must return a valid folder ID before the move proceeds.

Pass move OTIDs as separate arguments. The CLI removes duplicates and sends one comma-separated `speech_otid_list` form value: repeated form fields were observed to move only the last recording despite an `OK` response. Success now requires every requested OTID in `added_speech_otids`. Partial acknowledgement exits nonzero and lists confirmed/unconfirmed IDs; a missing or malformed acknowledgement reports unconfirmed completion. Reload affected recordings before retrying; moves are not automatically replayed.

`speeches download --output PATH` writes to that **exact path**, including when it has no extension. For example, `--format mp3 --output interview.mp3` produces `interview.mp3`. Without `--output`, the filename is `OTID.<format>`, or `OTID.zip` for multiple comma-separated formats. Older versions treated `--output` as a stem and appended an extension; include the extension yourself when upgrading scripts that relied on that behavior.

Uploads stream the audio file, and downloads stream into a temporary file beside the destination. A download replaces the destination only after the complete HTTP 200 export arrives. HTTP errors, partial responses, and interrupted transfers leave an existing destination unchanged. Temporary files are removed on handled failures. Export errors preserve server retry guidance, including non-JSON rate-limit responses.

## Tag selected speakers in one session

First identify the correct speaker and review the segment UUIDs. With no `-t` or `--all`, `speakers tag` only lists segments:

```bash
otter speakers list --json
otter speeches get OTID --json
otter speakers tag OTID SPEAKER_ID --json
```

Then pass all reviewed UUIDs in **one command**, instead of running a separate command for every segment:

```bash
otter speakers tag OTID SPEAKER_ID -t UUID1 -t UUID2 -t UUID3
# Comma-separated UUIDs work too:
otter speakers tag OTID SPEAKER_ID -t UUID1,UUID2,UUID3 --json
```

Both forms reuse one login, one speaker lookup, one transcript fetch, and one HTTP session for the batch. Each selected segment still requires its own tagging request. Duplicate UUIDs are removed, and the full selection is checked against the conversation before any changes are saved. Existing single-segment `-t UUID` commands still work.

`--all` assigns the selected speaker to **every segment in the conversation**, including other people's turns. It does not mean “all turns belonging to this person.” It cannot be combined with `-t`.

The batch stops on the first API or transport error and exits nonzero. Successful tags remain saved. JSON output reports `tagged_uuids`, `failed_uuid`, `unattempted_uuids`, `error`, and `retry_after_seconds`, along with the conversation and speaker IDs. A failed or interrupted network request may already have saved its change: reload the conversation before retrying that segment, then resume only the necessary IDs. The CLI does not automatically replay mutations.

Commands check Otter's JSON status as well as the HTTP status. An explicit non-`OK` API status fails even with HTTP 200, and malformed JSON success responses fail instead of becoming empty results. JSON export errors are reported before writing an output file. API and export HTTP errors retain their status and retry guidance when the server sends a non-JSON error page.

## Rate limits: findings and operating guidance

The CLI currently authenticates once per command invocation. Separate commands still log in separately; there is no session cache shared between processes. Repeated single-segment tagging therefore sends repeated `/login` requests, even though all tags could use one session. HTTP 429 can occur during login **before the requested operation runs**. Batching selected tags fixes this workflow without adding persistent session-cookie storage.

These are observations from actual cleanup runs, **not an official quota or a guaranteed reset schedule**:

- **June 2026:** a burst of roughly 12–15 authenticated invocations over a couple of minutes hit a login rate limit. A pause of about 2–3 minutes, followed by spacing commands roughly 30 seconds apart, allowed the remaining work to finish.
- **September 8, 2026:** after several listing/detail/speaker operations, repeated per-segment tag commands hit `/login` HTTP 429. The response included `{"status":"failed","message":"rate limited","retry_after":16}`. Reusing one authenticated session allowed the remaining selected-tag work and verification to complete. This does not imply a limit of 16 requests or a fixed 16-second reset window.

When automating:

1. Batch selected speaker tags with repeated `-t` flags or comma-separated UUIDs. Batch folder moves by passing multiple OTIDs.
2. On HTTP 429, stop. Honor the server's `Retry-After` header or JSON `retry_after` delay. API and export error messages surface the delay, and tag batch results include it as `retry_after_seconds`; if both are present, the longer delay is used.
3. If the server provides no delay, start with a 60–90 second pause, then retry slowly. A longer pause may be necessary. Do not run parallel retry loops or repeatedly log in to check whether the limit has cleared.
4. Preserve the batch result and reload affected segments before resuming after an error. Some tags may already be saved.

We have not established a requests-per-minute quota, whether all endpoints share a quota, or an exact reset policy. One-session batching reduces avoidable login requests; it does not remove Otter's rate limits on login or other endpoints. Keep this section updated with new observations without including private meeting content, account identifiers, credentials, or session tokens.

## Develop

```bash
cargo build
cargo test                  # live API tests skip unless OTTERAI_USERNAME/OTTERAI_PASSWORD are set
cargo fmt --all
cargo clippy --all-targets
```

When running live mutation tests, upload a throwaway file and trash it afterward.

The API is unofficial and drifts. Response JSON stays untyped on purpose. `finish_speech_upload` needs `appid=otter-web`. See the rate-limit findings above when scripting or testing against the live service.

## License

MIT. Originally based on [gmchad/otterai-api](https://github.com/gmchad/otterai-api); the last Python tree is the `python-final` tag. Keep the LICENSE file. Forks are welcome. All changes to this repository should be done through a pull request; anyone is free to make a pull request.
