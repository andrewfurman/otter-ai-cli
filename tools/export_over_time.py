#!/usr/bin/env python3
"""Run finite JSON archive batches hourly; stop when the snapshot is complete."""
import argparse
import json
from pathlib import Path
import subprocess
import time

from publish_archive_batch import publish


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--notes-repo', type=Path, required=True)
    parser.add_argument('--initial-delay-seconds', type=int, default=0)
    args = parser.parse_args()
    project = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    observations = project / 'docs' / 'export-rate-limit-observations.jsonl'
    repository = args.notes_repo.resolve()
    with (output / 'worker.log').open('a', buffering=1) as log:
        if args.initial_delay_seconds > 0:
            print(f'Waiting {args.initial_delay_seconds} seconds before the next batch.', file=log)
            time.sleep(args.initial_delay_seconds)
        consecutive_failures = 0
        while True:
            # Recover a previously interrupted PR publication before more API requests.
            publish(output, repository)
            print(f'Batch started at {int(time.time())}', file=log)
            result = subprocess.run([
                str(project / 'target/debug/otter'), 'speeches', 'export-json', str(output),
                '--source', 'all', '--delay', '10', '--max-new', '25',
            ], stdout=log, stderr=log)
            publish(output, repository)
            rate_file = output / 'rate-limits.jsonl'
            wait = 3600
            if rate_file.exists():
                # This log contains only UTC epochs, endpoint categories, HTTP statuses and delays.
                observations.write_text(rate_file.read_text())
                events = [json.loads(line) for line in rate_file.read_text().splitlines() if line.strip()]
                if events:
                    latest = events[-1]
                    wait = max(wait, latest['observed_at_unix_seconds'] + (latest.get('retry_after_seconds') or 90) - time.time())
            manifest_path = output / 'manifest.json'
            manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
            if result.returncode == 0 and (manifest.get('complete') or manifest.get('finished')):
                print(f"Archive traversal finished; unavailable={manifest.get('unavailable', 0)}; complete={manifest.get('complete')}. Worker stopped.", file=log)
                return
            # Retry read failures only on the hourly cadence, with a finite failure budget.
            if result.returncode != 0:
                recent = (output / 'rate-limits.jsonl').read_text() if rate_file.exists() else ''
                events = [json.loads(line) for line in recent.splitlines() if line.strip()]
                consecutive_failures += 1
                if consecutive_failures >= 3:
                    raise SystemExit('Three consecutive failed batches; see worker.log and manifest.json')
            else:
                consecutive_failures = 0
            print(f'Next batch in {int(wait)} seconds.', file=log)
            time.sleep(wait)


if __name__ == '__main__':
    main()
