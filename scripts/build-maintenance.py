#!/usr/bin/env python3
"""Derive the response CSP from the canonical, self-contained maintenance page."""
import argparse
from pathlib import Path
import sys
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent / 'release'))
from update_coordinator import maintenance_page

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
_, policy = maintenance_page()
path = Path(__file__).resolve().parents[1] / 'assets/maintenance.csp'
if args.check:
    if path.read_text() != policy:
        raise SystemExit('Maintenance CSP changed: run python3 scripts/build-maintenance.py')
else:
    path.write_text(policy)
