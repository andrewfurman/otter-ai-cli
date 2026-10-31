# Structured transcript archive

Export a finite snapshot of owned and shared conversations incrementally:

```bash
cargo build
./target/debug/otter speeches export-json '/path/to/Otter Transcripts' --source all --delay 10 --max-new 25
```

The command logs in once per batch. It caches a complete paginated listing, sorts it newest first by recording date (using the supported ET title suffix for imported recordings, then displayed/start time, then creation time), fetches up to 25 new conversations, reuses validated existing files, and publishes indexes for everything exported so far. Re-running the command continues the same snapshot. It does not refresh previously exported transcripts; use a new destination for a fresh snapshot. It stops on the first API error without an immediate retry. Inspect `manifest.json` for completeness; exit zero can indicate a successful partial batch.

`python3 tools/export_over_time.py OUTPUT --notes-repo /path/to/Legacy-Notes-Archive` runs these batches one hour apart and stops when the snapshot is complete. Requests are spaced ten seconds apart, including listing pages. These are conservative operating choices, not measured Otter quotas. Only run one worker against a destination. Non-rate-limit errors stop the worker for investigation.

Each conversation file contains:

- OTID, title, creation and recording timestamps (both numeric and ISO 8601 UTC), timezone, duration and speaker list. `recording_start_iso_utc` reflects Otter's `start_time`; `displayed_start_iso_utc` preserves its displayed date separately. The separate `recorded_at_iso_utc` field uses a supported ET date suffix in the title when present, then displayed/start time, then creation time; `recorded_at_source` records the choice. Unsupported title date formats are not guessed.
- `segments`: speaker ID and resolved name, text, start/end milliseconds, seconds and `HH:MM:SS.mmm` timestamps relative to recording start. Missing names or timing remain null.
- `raw_segment`: original segment fields, including word alignment when returned by Otter. Alignment fields are preserved without assuming their time base.
- `text`: joined transcript text.
- `raw_response`: supplementary API metadata, excluding transient tokens, session fields and URL fields. Transcript content and original segment data remain intact.

`index.jsonl` has one searchable row per exported conversation. `segments.jsonl` has one row per segment with conversation ID/title, speaker, text and timestamps. These indexes contain private transcript content and belong with the notes archive, outside the CLI source repository.

```bash
# Find every matching turn, with speaker and position in the recording:
jq -c 'select(.text | test("renewal"; "i")) | {title, speaker_name, start_timestamp, text}' '/path/to/Otter Transcripts/segments.jsonl'
# Find turns by speaker name:
jq -c 'select((.speaker_name // "") | test("Andrew"; "i"))' '/path/to/Otter Transcripts/segments.jsonl'
```

## Rate-limit observations

The exporter records actual HTTP 429 events in `OUTPUT/rate-limits.jsonl`, including observation time as UTC Unix seconds, endpoint category and the server-provided retry delay, if present. No account identifiers, transcript content or response bodies are included. The worker copies this evidence to `docs/export-rate-limit-observations.jsonl` in this project when events occur.

On 429 the current batch stops, preserving completed conversation files. The worker waits at least one hour and honors any longer server delay before resuming. It does not probe login repeatedly. Initial setup on 2026-10-31 verified one listing and one conversation detail successfully; no rate limit was observed in those two checks. The first exploratory bulk listing was interrupted to switch to paced batches; it did not establish an archive size or quota. See the README for older observed limits; none are official guarantees.

All source changes and future archive additions go through their respective repositories' pull requests. No automatic Git commit or push occurs.

## Pull requests to the notes archive

The hourly worker publishes every batch to an independent branch and PR against `andrewfurman/Legacy-Notes-Archive:main`. It never merges a PR or pushes directly to main. Each PR adds its own directory under `Legacy Otter Transcripts/batches/TIMESTAMP/`, with up to 25 conversations, indexes, a manifest and schema notes. Because these directories are independent, PRs can be reviewed in any order. Search all merged batches with:

```bash
jq -c 'select(.text | test("renewal"; "i")) | {title, recorded_at_iso_utc, speaker_name, start_timestamp, text}' "Legacy Otter Transcripts"/batches/*/segments.jsonl
```

`OUTPUT/pull-requests.json` records submitted IDs and PR URLs, including a pending-publication journal for recovery. Completed local exports are reused on restart. Pending publication is recovered before exporting more conversations. A failure during GitHub publication stops the worker for investigation rather than creating another branch. Keep this state alongside the local archive to avoid resubmitting conversations whose PRs are still open. Closed PRs remain recorded as submitted.

This is a finite snapshot of accessible conversations, not an ongoing sync of future recordings. Conversations Otter refuses to return stop the current run and remain visible in the manifest; an inaccessible conversation is never counted as exported. The background service runs while this VM is running; transient service configuration does not survive VM reboots.
