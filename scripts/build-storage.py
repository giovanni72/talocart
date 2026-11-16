#!/usr/bin/env python3
"""Read-only build storage budget; never deletes or follows symbolic links."""
import argparse
import json
import math
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
GIB = 1024 ** 3


def allocated_bytes(root):
    if root.is_symlink():
        raise ValueError('Refusing a symbolic-link build directory')
    if not root.exists():
        return 0
    seen = set()
    pending = [root]
    total = 0
    while pending:
        with os.scandir(pending.pop()) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    pending.append(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    stat = entry.stat(follow_symlinks=False)
                    identity = (stat.st_dev, stat.st_ino)
                    if identity not in seen:
                        seen.add(identity)
                        total += stat.st_blocks * 512
    return total


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--max-gib', type=float, default=40,
                        help='Combined debug/debug-full budget (default: 40 GiB)')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    if not math.isfinite(args.max_gib) or args.max_gib <= 0:
        parser.error('--max-gib must be positive and finite')
    try:
        sizes = {name: allocated_bytes(ROOT / 'target' / name)
                 for name in ('debug', 'debug-full')}
    except (OSError, ValueError):
        print('Storage check failed; retry with no build/cleanup in progress. / '
              'Verifica spazio fallita; riprovare senza build/pulizie in corso.', file=sys.stderr)
        return 2
    total = sum(sizes.values())
    exceeded = total > args.max_gib * GIB
    report = {'allocated_bytes': sizes, 'total_gib': round(total / GIB, 3),
              'limit_gib': args.max_gib, 'over_budget': exceeded}
    if args.json:
        print(json.dumps(report))
    else:
        print(f'Debug: {total / GIB:.2f} GiB / budget {args.max_gib:g} GiB')
        print('Budget exceeded: plan a cleanup between sessions. / '
              'Soglia superata: pianificare pulizia tra sessioni.' if exceeded else
              'Within budget. / Entro la soglia.')
    return 1 if exceeded else 0


if __name__ == '__main__':
    sys.exit(main())
