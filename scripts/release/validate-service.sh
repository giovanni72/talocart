#!/bin/sh
# CI-only: validate examples in a disposable Linux container, never on a host.
set -eu
[ "${1:-}" = '--disposable-linux-container' ]
[ -f /.dockerenv ]
release_dir=$2
[ ! -e /opt/talocart ]
[ ! -e /etc/letsencrypt ]
mkdir -p /opt/talocart/current/bin /var/lib/talocart /etc/letsencrypt/live/shop.example.com
cp "$release_dir/bin/talocart" /opt/talocart/current/bin/talocart
cp "$release_dir/distribution/talocart.service" /tmp/talocart.service
systemd-analyze verify --man=no /tmp/talocart.service
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=shop.example.com \
  -keyout /etc/letsencrypt/live/shop.example.com/privkey.pem \
  -out /etc/letsencrypt/live/shop.example.com/fullchain.pem >/dev/null 2>&1
printf 'events {}\nhttp { include %s/distribution/nginx.conf; }\n' "$release_dir" > /tmp/talocart-nginx.conf
nginx -t -c /tmp/talocart-nginx.conf
