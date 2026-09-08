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
| `otter speeches list` | List conversations; `--days N` fetches its date window and `--all` fetches all pages |
| `otter speeches get OTID` | Fetch a conversation and its transcript segments |
| `otter speeches search QUERY OTID` | Search a transcript, optionally filtering by speaker |
| `otter speeches rename OTID TITLE` | Change a conversation title |
| `otter speeches rename-batch PLAN.json` | Validate a rename plan, reuse one login, and verify saved titles; `--dry-run` previews offline |
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

`speeches list` defaults to one page (`--page-size 45`) with an incompleteness notice when more results may exist. `--days N` automatically follows pages until its creation-time window is covered; `--all` fetches the full archive, or the complete date window when combined with `--days`. Folder and source filters stay on every request; speaker filtering happens after collection. No matches on the default single page does not establish that no matching conversation exists.

Pagination reuses one login, follows Otter's `last_load_ts` cursor with `modified_after=1`, and removes overlapping results by OTID, keeping the first observed copy. Requests of 100 per page worked in the September 2026 archive review; much larger requests timed out. `--page-size` is a per-request size, subject to Otter's behavior, not a total result limit. `--days` retains the existing `created_at` semantics (creation/upload time, which can differ from recording time).

JSON includes `pagination.complete`, `scope`, `created_after`, `pages_fetched`, `unique_fetched`, `stop_reason`, `error`, and `retry_after_seconds`. Completeness refers to the requested archive/date window before speaker filtering. Otter's last-page `end_of_list` is retained, so it can be false when the requested date window is complete. Pagination stops without retries on API/transport failures, malformed pages, missing/non-advancing cursors, or `--max-pages` (default 1000). These failures retain collected results, mark completion false, and exit nonzero. Date-window coverage relies on the observed cursor being the next historical page's creation-time upper bound; this is an unofficial API, not a transactional snapshot.

```bash
otter speeches list --days 14 --page-size 100 --json
otter speeches list --all --page-size 100 --speaker "Alice" --json
```

`speeches move --create` creates a folder only when a successful folder lookup confirms that its name is missing. A failed lookup stops the command before any folder creation or moves, including permission, rate-limit, network, and malformed-response errors. Creation must return a valid folder ID before the move proceeds.

Pass move OTIDs as separate arguments. The CLI removes duplicates and sends one comma-separated `speech_otid_list` form value: repeated form fields were observed to move only the last recording despite an `OK` response. Success now requires every requested OTID in `added_speech_otids`. Partial acknowledgement exits nonzero and lists confirmed/unconfirmed IDs; a missing or malformed acknowledgement reports unconfirmed completion. Reload affected recordings before retrying; moves are not automatically replayed.

`speeches download --output PATH` writes to that **exact path**, including when it has no extension. For example, `--format mp3 --output interview.mp3` produces `interview.mp3`. Without `--output`, the filename is `OTID.<format>`, or `OTID.zip` for multiple comma-separated formats. Older versions treated `--output` as a stem and appended an extension; include the extension yourself when upgrading scripts that relied on that behavior.

Uploads stream the audio file, and downloads stream into a temporary file beside the destination. A download replaces the destination only after the complete HTTP 200 export arrives. HTTP errors, partial responses, and interrupted transfers leave an existing destination unchanged. Temporary files are removed on handled failures. Export errors preserve server retry guidance, including non-JSON rate-limit responses.

## Rename recordings in one session

Save a JSON array with each recording's OTID, exact current title, and proposed title. Use `null` for an untitled recording; `old_title` is required so a stale plan cannot silently overwrite a title that already changed. Unknown fields, duplicate OTIDs, missing fields, and blank new titles fail before authentication.

```json
[
  {"otid": "OTID1", "old_title": "Weekly Meeting", "new_title": "Project Weekly Sync on Tue Sep 8th 2026 @ 10:00am ET"},
  {"otid": "OTID2", "old_title": null, "new_title": "Project Planning on Tue Sep 8th 2026 @ 11:00am ET"}
]
```

```bash
otter speeches rename-batch plan.json --dry-run --json
otter speeches rename-batch plan.json --json > result.json
```

Preview only validates and displays the local plan; it does not log in or check current Otter titles. Apply logs in once, reads each recording, skips an already-correct title, checks `old_title`, renames it, and reads it back before counting it as saved. The check and write are separate requests, so this is not an atomic lock against simultaneous edits. Each changed recording uses three API requests, each already-correct recording uses one, and every request shares the same authenticated session.

The first conflict, API/transport error, or failed verification stops the batch without retries and exits nonzero. Progress goes to stderr; `--json` returns one report on stdout with `saved_otids`, `unchanged_otids`, `failed_otid`, `unattempted_otids`, `error`, `retry_after_seconds`, and `unconfirmed_write`. A sent rename can have saved even when its response or verification fails. Reload that recording before retrying. Rerunning a reviewed plan skips already-correct titles and stops on conflicts. Keep the original plan for your audit trail; completed changes are not automatically rolled back. Invalid plans and login failures exit before a batch report is available.

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

Login requires a valid user ID; folder/speaker lists require arrays of objects, and conversation details must identify the requested OTID. A missing or malformed required field exits nonzero with an unexpected-response error indicating that the API may have changed. Empty lists, extra fields, and optional conversation metadata remain supported. Writes require an explicit `status: OK` acknowledgement, and folder creation must return a valid folder ID. A missing acknowledgement leaves completion unconfirmed: reload affected data before retrying. No automatic mutation retries are added.

Exports served as `text/html` or `application/xhtml+xml` are rejected before creating or replacing a file, even with HTTP 200. Detection uses the response's Content-Type; ordinary transcript text containing HTML or JSON remains valid. This catches declared HTML login/error pages, without attempting to guess every possible API change or incorrectly labelled response.

## Rate limits: findings and operating guidance

The CLI authenticates once per command invocation. Paginated listings, batch renames, bulk moves, and selected speaker tags each reuse that invocation's session. Separate commands still log in separately; there is no session cache shared between processes. HTTP 429 can occur during login **before the requested operation runs**. Use the batch commands to reduce avoidable logins.

These are observations from actual cleanup runs, **not an official quota or a guaranteed reset schedule**:

- **June 2026:** a burst of roughly 12–15 authenticated invocations over a couple of minutes hit a login rate limit. A pause of about 2–3 minutes, followed by spacing commands roughly 30 seconds apart, allowed the remaining work to finish.
- **September 8, 2026:** after several listing/detail/speaker operations, repeated per-segment tag commands hit `/login` HTTP 429. The response included `{"status":"failed","message":"rate limited","retry_after":16}`. Reusing one authenticated session allowed the remaining selected-tag work and verification to complete. This does not imply a limit of 16 requests or a fixed 16-second reset window.

When automating:

1. Batch selected speaker tags with repeated `-t` flags or comma-separated UUIDs, folder moves with multiple OTIDs, and title changes with `speeches rename-batch`. Listing pages share a login automatically with `--days` or `--all`.
2. On HTTP 429, stop. Honor the server's `Retry-After` header or JSON `retry_after` delay. API and export error messages surface the delay, and tag batch results include it as `retry_after_seconds`; if both are present, the longer delay is used.
3. If the server provides no delay, start with a 60–90 second pause, then retry slowly. A longer pause may be necessary. Do not run parallel retry loops or repeatedly log in to check whether the limit has cleared.
4. Preserve the batch result and reload affected segments before resuming after an error. Some tags may already be saved.

We have not established a requests-per-minute quota, whether all endpoints share a quota, or an exact reset policy. One-session batching reduces avoidable login requests; it does not remove Otter's rate limits on login or other endpoints. Keep this section updated with new observations without including private meeting content, account identifiers, credentials, or session tokens.

## Develop

```bash
cargo build
cargo test                  # offline suite; reports the live smoke test as ignored
cargo fmt --all
cargo clippy --all-targets
```

The live smoke test is explicitly ignored by default, even when credentials are present. To run it, securely supply nonempty `OTTERAI_USERNAME` and `OTTERAI_PASSWORD` environment variables, then run:

```bash
cargo test --test live -- --ignored --nocapture
```

This single read-only test logs in once and checks the account, conversations, folders, speakers, and groups sequentially through the same client. It stops on the first failed check without retrying. HTTP/API failures show the failing stage, HTTP status, and retry delay when available; raw response bodies and credentials are not printed. An explicit run without credentials fails before any requests instead of silently passing. Follow the rate-limit guidance above before rerunning a failed smoke test. It does not read saved CLI credentials automatically or change any recordings.

When running live mutation tests, upload a throwaway file and trash it afterward.

The API is unofficial and drifts. Response JSON stays untyped on purpose. `finish_speech_upload` needs `appid=otter-web`. See the rate-limit findings above when scripting or testing against the live service.

## License

MIT. Originally based on [gmchad/otterai-api](https://github.com/gmchad/otterai-api); the last Python tree is the `python-final` tag. Keep the LICENSE file. Forks are welcome. All changes to this repository should be done through a pull request; anyone is free to make a pull request.
