import gzip
from pathlib import Path
import tempfile
import unittest

from compact_local_archive import compact


class CompactionTest(unittest.TestCase):
    def test_exact_original_bytes_survive_compaction_and_repeat_is_safe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'conversations').mkdir()
            source = root / 'conversations/61.json'
            raw = '{"otid":"a","text":"café","segments":[]}\n'.encode()
            source.write_bytes(raw)
            self.assertEqual(compact(root), 1)
            self.assertFalse(source.exists())
            self.assertEqual(gzip.decompress(source.with_suffix('.json.gz').read_bytes()), raw)
            self.assertEqual(compact(root), 0)

    def test_conflicting_compressed_copy_does_not_remove_original(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'conversations').mkdir()
            source = root / 'conversations/61.json'
            source.write_bytes(b'{"original":true}')
            source.with_suffix('.json.gz').write_bytes(gzip.compress(b'wrong data'))
            with self.assertRaises(RuntimeError):
                compact(root)
            self.assertEqual(source.read_bytes(), b'{"original":true}')


if __name__ == '__main__':
    unittest.main()
