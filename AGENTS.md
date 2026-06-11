# otter-ai-cli

Unofficial Otter.ai CLI. Single Rust crate: `src/main.rs` + per-command modules
are the CLI; `src/client.rs` is the API client and the reference for every
known Otter.ai endpoint; `src/config.rs` stores credentials in
`~/.otterai/config.json` (env vars `OTTERAI_USERNAME`/`OTTERAI_PASSWORD` take
precedence).

## Commands

```bash
cargo build                  # debug build
cargo test                   # unit tests; live API tests skip without creds
cargo fmt --all              # format (CI enforces --check)
cargo clippy --all-targets   # lint (CI enforces -D warnings)
cargo install --path .       # install/update the `otter` binary
```

Run `otter --help` (and `otter <group> --help`) for the full command surface;
most commands take `--json` for machine-readable output.

## Live testing

`tests/live.rs` hits the real Otter.ai API and self-skips unless
`OTTERAI_USERNAME`/`OTTERAI_PASSWORD` are set. When verifying mutations
end-to-end, upload a throwaway audio file and trash it afterwards — never
mutate real meetings.

## Gotchas (not inferable from the code)

- Speeches have two IDs. The API only accepts **`otid`**
  (e.g. `jqb7OHo6mrHtCuMkyLN0nUS8mxY`); `speech_id` (e.g. `22WB27HAEBEJYFCA`)
  does not work. All CLI `SPEECH_ID` args mean otid.
- The API is unofficial and drifts without notice. Past breakage:
  `finish_speech_upload` started requiring `appid=otter-web`; the speakers
  payload renamed `speaker_id` to `id`. When a request 400s, suspect drift and
  compare with what the otter.ai web app sends.
- `ApiResponse.data` is deliberately untyped (`serde_json::Value`) because of
  that drift — don't introduce strict response structs.
- The project descends from gmchad/otterai-api (Python, MIT). The final Python
  implementation is at the `python-final` git tag; the MIT LICENSE file must
  stay.
