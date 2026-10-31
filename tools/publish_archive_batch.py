"""Publish exported conversations in independent, resumable archive pull requests."""
from datetime import datetime, timezone
import json
from pathlib import Path
import shutil
import subprocess

REPOSITORY = 'andrewfurman/Legacy-Notes-Archive'


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def save(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def publish(output, repository):
    state_path = output / 'pull-requests.json'
    state = json.loads(state_path.read_text()) if state_path.exists() else {'published_otids': [], 'pull_requests': []}
    if 'pending' not in state:
        published = set(state['published_otids'])
        docs = []
        for path in (output / 'conversations').glob('*.json'):
            doc = json.loads(path.read_text())
            if doc['otid'] not in published:
                docs.append((path, doc))
        docs.sort(key=lambda pair: (-(pair[1].get('recorded_at_unix_seconds') or 0), pair[1]['otid']))
        docs = docs[:25]
        if not docs:
            return
        if command('git', 'status', '--porcelain', cwd=repository):
            raise RuntimeError('Archive checkout has local changes; refusing to overwrite them')
        command('git', 'fetch', 'origin', 'main', cwd=repository)
        batch = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        branch = f'otter-archive/{batch}'
        relative = f'Legacy Otter Transcripts/batches/{batch}'
        # Persist intent before any network write for idempotent recovery.
        state['pending'] = {'branch': branch, 'directory': relative, 'files': [str(p) for p, _ in docs]}
        save(state_path, state)
        command('git', 'switch', '-c', branch, 'origin/main', cwd=repository)
    pending = state['pending']
    branch = pending['branch']
    if command('git', 'branch', '--show-current', cwd=repository) != branch:
        raise RuntimeError('Pending batch branch is not checked out; inspect pull-requests.json')
    target = repository / pending['directory']
    target.mkdir(parents=True, exist_ok=True)
    (target / 'conversations').mkdir(exist_ok=True)
    docs = []
    for filename in pending['files']:
        source = Path(filename)
        doc = json.loads(source.read_text())
        if not isinstance(doc.get('segments'), list) or not isinstance(doc.get('text'), str):
            raise RuntimeError('Invalid conversation export')
        shutil.copyfile(source, target / 'conversations' / source.name)
        docs.append((source, doc))
    with (target / 'index.jsonl').open('w') as index, (target / 'segments.jsonl').open('w') as segments:
        for source, doc in docs:
            relative_path = f'conversations/{source.name}'
            row = {key: doc.get(key) for key in ['otid', 'title', 'recorded_at_iso_utc', 'recorded_at_source', 'created_at_iso_utc', 'duration_seconds', 'speakers', 'text']}
            row['path'] = relative_path
            index.write(json.dumps(row, ensure_ascii=False) + '\n')
            for segment in doc['segments']:
                row = {key: value for key, value in segment.items() if key != 'raw_segment'}
                row.update(otid=doc['otid'], title=doc['title'], recorded_at_iso_utc=doc.get('recorded_at_iso_utc'), path=relative_path)
                segments.write(json.dumps(row, ensure_ascii=False) + '\n')
    dates = sorted(doc['recorded_at_iso_utc'] for _, doc in docs if doc.get('recorded_at_iso_utc'))
    date_range = f'{dates[0][:10]} to {dates[-1][:10]}' if dates else 'date unknown'
    manifest = {'schema_version': 1, 'count': len(docs), 'date_range': date_range,
                'order': 'recorded_at_desc', 'otids': [doc['otid'] for _, doc in docs]}
    progress_path = output / 'manifest.json'
    if progress_path.exists():
        progress = json.loads(progress_path.read_text())
        manifest['inventory_complete'] = progress.get('listing_complete', False)
        manifest['known_conversations'] = progress.get('total')
    unavailable_path = output / 'unavailable.json'
    if unavailable_path.exists():
        unavailable = json.loads(unavailable_path.read_text())
        manifest['unavailable_conversations'] = len(unavailable)
        save(target / 'unavailable.json', unavailable)
    save(target / 'manifest.json', manifest)
    (target / 'README.md').write_text(
        f'# Otter transcript batch\n\n{len(docs)} conversations, {date_range}, newest first.\n\n'
        'Each conversation includes title, recorded date and date source, speaker IDs and names, '
        'segment text, recording-relative timestamps, and original word alignments when available. '
        'Title dates in the CLI-supported Eastern Time format take precedence over displayed/start dates '
        'and upload dates. Unknown dates and speakers remain null.\n\n'
        '`index.jsonl` contains one row per conversation; `segments.jsonl` contains one row per turn. '
        'Both use paths relative to this batch directory. Transient tokens, session fields and URL '
        'fields are omitted from supplementary API metadata. This batch is part of an incremental '
        'archive; it does not claim to contain the entire Otter account.\n')
    command('git', 'add', '--', pending['directory'], cwd=repository)
    if command('git', 'diff', '--cached', '--name-only', cwd=repository):
        command('git', 'commit', '-m', f'Archive {len(docs)} Otter conversations ({date_range})', cwd=repository)
    command('git', 'push', '-u', 'origin', branch, cwd=repository)
    existing = json.loads(command('gh', 'pr', 'list', '--repo', REPOSITORY, '--head', branch, '--state', 'all', '--json', 'url'))
    if existing:
        url = existing[0]['url']
    else:
        body = output / 'pending-pr-body.txt'
        body.write_text(
            f'Archive {len(docs)} Otter conversations from {date_range} as structured JSON, working backward through the account.\n\n'
            'Includes conversation titles and dates, speaker-resolved segments with millisecond/second/readable timestamps, '
            'and searchable conversation and segment JSONL indexes. Original transcript and alignment fields are preserved; '
            'transient access metadata is omitted.\n\n'
            'Validation: all conversation JSON parsed and segment/text fields checked; indexes generated from the same documents. '
            'Stable Otter IDs are recorded to prevent resubmission across batches. This independent batch targets main; no automatic merge.\n')
        url = command('gh', 'pr', 'create', '--repo', REPOSITORY, '--base', 'main', '--head', branch,
                      '--title', f'Archive Otter transcripts: {date_range} ({len(docs)} conversations)', '--body-file', str(body))
    state['published_otids'] = sorted(set(state['published_otids']) | set(manifest['otids']))
    state['pull_requests'].append({'url': url, **manifest})
    del state['pending']
    save(state_path, state)
    print(f'Published {url}', flush=True)
