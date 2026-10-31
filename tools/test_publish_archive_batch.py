import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import publish_archive_batch as publisher


class BatchPublicationTest(unittest.TestCase):
    def test_real_git_publication_is_deduplicated_and_indexes_match_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            remote, checkout, output = root / 'remote.git', root / 'notes', root / 'output'
            real_command = publisher.command
            real_command('git', 'init', '--bare', str(remote))
            real_command('git', 'clone', str(remote), str(checkout))
            real_command('git', 'config', 'user.name', 'Test', cwd=checkout)
            real_command('git', 'config', 'user.email', 'test@example.invalid', cwd=checkout)
            real_command('git', 'switch', '-c', 'main', cwd=checkout)
            (checkout / 'README.md').write_text('Existing notes\n')
            real_command('git', 'add', '.', cwd=checkout)
            real_command('git', 'commit', '-m', 'Initial notes', cwd=checkout)
            real_command('git', 'push', '-u', 'origin', 'main', cwd=checkout)
            (output / 'conversations').mkdir(parents=True)
            for i in range(2):
                doc = {'otid': str(i), 'title': 'Call', 'recorded_at_unix_seconds': i + 1,
                       'recorded_at_iso_utc': f'2026-10-0{i+1}T00:00:00+00:00', 'text': 'hello',
                       'segments': [{'text': 'hello', 'speaker_name': 'Alice', 'start_ms': 1000, 'raw_segment': {}}]}
                (output / 'conversations' / f'{i}.json').write_text(json.dumps(doc))
            creates = []

            def command(*args, cwd=None):
                if args[:3] == ('gh', 'pr', 'list'):
                    return '[]'
                if args[:3] == ('gh', 'pr', 'create'):
                    creates.append(args)
                    return 'https://example.invalid/pull/1'
                return real_command(*args, cwd=cwd)

            with patch.object(publisher, 'command', command), contextlib.redirect_stdout(io.StringIO()):
                publisher.publish(output, checkout)
                publisher.publish(output, checkout)
            self.assertEqual(len(creates), 1)
            state = json.loads((output / 'pull-requests.json').read_text())
            self.assertEqual(state['published_otids'], ['0', '1'])
            self.assertNotIn('pending', state)
            batch = next((checkout / 'Legacy Otter Transcripts/batches').iterdir())
            rows = [json.loads(line) for line in (batch / 'index.jsonl').read_text().splitlines()]
            self.assertEqual([row['otid'] for row in rows], ['1', '0'])
            for row in rows:
                self.assertTrue((batch / row['path']).is_file())
            segments = [json.loads(line) for line in (batch / 'segments.jsonl').read_text().splitlines()]
            self.assertEqual(segments[0]['speaker_name'], 'Alice')
            self.assertNotIn('raw_segment', segments[0])
            self.assertEqual(real_command('git', 'status', '--porcelain', cwd=checkout), '')
            self.assertNotIn('Legacy Otter', real_command('git', 'ls-tree', '--name-only', 'main', cwd=checkout))


if __name__ == '__main__':
    unittest.main()
