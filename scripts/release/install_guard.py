#!/usr/bin/python3 -I
"""Systemd lifetime guard for first installation; independent of app/updater code."""
import fcntl
import json
import os
from pathlib import Path
import socket
import stat
import time

STATE = Path('/var/lib/talocart-update')
UNIT = '''[Unit]
Description=Talocart first-installation guard
[Service]
Type=notify
RemainAfterExit=yes
ExecStart=/usr/bin/python3 -I /var/lib/talocart-update/install_guard.py
TimeoutStartSec=15
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
RestrictAddressFamilies=AF_UNIX
'''


def trusted_open(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
        os.close(fd)
        raise ValueError('Untrusted installation state')
    return fd


def phase():
    with os.fdopen(trusted_open(STATE/'installation.json')) as stream:
        value = json.loads(stream.read(4097))
    if value.get('format') != 1 or value.get('phase') not in ('installing', 'succeeded', 'failed'):
        raise ValueError('Invalid installation journal')
    return value['phase']


def check(fd):
    status = phase()
    if status == 'succeeded':
        return True
    if status != 'installing':
        raise ValueError('Installation failed')
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return False  # Installer still owns its exclusive, non-inherited lease.
    fcntl.flock(fd, fcntl.LOCK_UN)
    # Completion can race the first read and lease release.
    if phase() == 'succeeded':
        return True
    raise ValueError('Installation was interrupted; administrative recovery required')


def main():
    fd = trusted_open(STATE/'installation.lock')
    try:
        complete = check(fd)
        address = os.environ['NOTIFY_SOCKET']
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notify:
            notify.connect('\0'+address[1:] if address.startswith('@') else address)
            notify.sendall(b'READY=1')
        while not complete:
            time.sleep(0.2)
            complete = check(fd)
        # RemainAfterExit keeps the dependency active after durable success.
    finally:
        os.close(fd)


if __name__ == '__main__':
    main()
