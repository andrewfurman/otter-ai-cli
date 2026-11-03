#!/usr/bin/env python3
"""Losslessly compress local conversation copies; verify each before replacing it."""
import argparse
import gzip
import os
from pathlib import Path


def compact(output):
    count = before = after = 0
    for source in sorted((output / 'conversations').glob('*.json')):
        raw = source.read_bytes()
        destination = source.with_suffix('.json.gz')
        if destination.exists():
            if gzip.decompress(destination.read_bytes()) != raw:
                raise RuntimeError(f'Existing compressed file differs: {source.name}')
        else:
            temporary = destination.with_suffix('.gz.tmp')
            temporary.write_bytes(gzip.compress(raw, compresslevel=3, mtime=0))
            os.chmod(temporary, 0o600)
            if gzip.decompress(temporary.read_bytes()) != raw:
                raise RuntimeError(f'Compression verification failed: {source.name}')
            temporary.replace(destination)
        # The full original bytes have been verified in the compressed copy.
        before += len(raw)
        after += destination.stat().st_size
        source.unlink()
        count += 1
        if count % 100 == 0:
            print(f'Verified and compressed {count} conversations; saved {(before-after)/1024**2:.0f} MiB', flush=True)
    print(f'Finished: {count} conversations; saved {(before-after)/1024**3:.2f} GiB', flush=True)
    return count


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    compact(parser.parse_args().output)
