from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

import export_over_time as worker


class DiskReserveTest(unittest.TestCase):
    def test_low_space_stops_before_publication_or_api_requests(self):
        with tempfile.TemporaryDirectory() as directory:
            arguments = ['export_over_time.py', directory, '--notes-repo', directory]
            with patch('sys.argv', arguments), patch.object(worker.shutil, 'disk_usage', return_value=SimpleNamespace(free=0)), patch.object(worker, 'publish') as publish, patch.object(worker.subprocess, 'run') as run:
                with self.assertRaisesRegex(SystemExit, 'Export paused'):
                    worker.main()
                publish.assert_not_called()
                run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
