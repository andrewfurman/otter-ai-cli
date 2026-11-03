#!/usr/bin/env python3
"""Run finite JSON archive batches at a configurable interval; stop when the snapshot is complete."""
import argparse
import json
from pathlib import Path
import subprocess
import shutil
import time

from publish_archive_batch import publish


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--notes-repo', type=Path, required=True)
    parser.add_argument('--initial-delay-seconds', type=int, default=0)
    parser.add_argument('--interval-seconds', type=int, default=600)
    parser.add_argument('--min-free-mib', type=int, default=1536)
    parser.add_argument('--sparse-checkout', action=argparse.BooleanOptionalAction, default=True)
    args = parser.parse_args()
    if args.interval_seconds < 1 or args.initial_delay_seconds < 0 or args.min_free_mib < 1:
        parser.error('interval and minimum free space must be positive; initial delay must be nonnegative')
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
            # Preserve a reserve for publication, index replacement and checkpoints.
            if shutil.disk_usage(output).free < args.min_free_mib * 1024**2:
                raise SystemExit(f'Export paused: less than {args.min_free_mib} MiB free; saved exports and publication state preserved')
            # Recover a previously interrupted PR publication before more API requests.
            publish(output, repository, sparse=args.sparse_checkout)
            if shutil.disk_usage(output).free < args.min_free_mib * 1024**2:
                raise SystemExit(f'Export paused: less than {args.min_free_mib} MiB free; saved exports and publication state preserved')
            print(f'Batch started at {int(time.time())}', file=log)
            result = subprocess.run([
                str(project / 'bin/otter'), 'speeches', 'export-json', str(output),
                '--source', 'all', '--delay', '10', '--max-new', '25',
            ], stdout=log, stderr=log)
            publish(output, repository, sparse=args.sparse_checkout)
            rate_file = output / 'rate-limits.jsonl'
            wait = args.interval_seconds
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
            # Retry read failures only on the configured cadence, with a finite failure budget.
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
