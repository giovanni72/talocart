#!/usr/bin/env python3
"""Test-only adapter for PostgreSQL 17 clients in an existing test container."""
import os
from pathlib import Path
import re
import subprocess
import sys

container = os.environ['RELEASE_TEST_PG_CONTAINER']
assert re.fullmatch(r'[A-Za-z0-9_.-]+', container)
program, *args = sys.argv[1:]
assert program in ('psql', 'pg_dump', 'pg_restore')
command = ['docker', 'exec', '-i']
for key in ('PGUSER', 'PGPASSWORD', 'PGDATABASE', 'PGSSLMODE', 'PGCONNECT_TIMEOUT'):
    if key in os.environ:
        command += ['--env', key]
command += ['--env', 'PGHOST=127.0.0.1', '--env', 'PGPORT=5432', container, program]
if program == 'pg_dump':
    index = args.index('--file')
    output = Path(args[index + 1])
    del args[index:index + 2]
    with output.open('xb') as stream:
        result = subprocess.run(command + args, stdout=stream, check=False)
elif program == 'pg_restore':
    source = Path(args.pop())
    with source.open('rb') as stream:
        result = subprocess.run(command + args, stdin=stream, check=False)
else:
    result = subprocess.run(command + args, check=False)
sys.exit(result.returncode)
