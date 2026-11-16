# Talocart administrative bootstrap

Reviewed source: {{source_tag}}, commit {{source_commit}}.

Verify the detached signature with an independently trusted root public key
before extracting into root-owned non-writable ancestors. This administrative
kit is not a normal application update; the ordinary launcher rejects it.
It contains no private keys, tokens or store data.

A compatible base must already expose requests/control.sock with updates enabled.
The kit does not deploy the application or migrate its database. Stage a private
0600 TOML from update-unified.toml, inventory all writers and assert writers_fenced
only after fencing them. Configure the Nginx maintenance include and clear the
X-Talocart-Update-Probe header in every proxied application location.

Run preview as root, with separately verified engine and detached signature:

```sh
/usr/bin/python3 -I bootstrap_updater.py --config-source /root/staging-update.toml --engine /root/talocart-updater-ENGINE_VERSION.tar.gz --signature /root/talocart-updater-ENGINE_VERSION.tar.gz.sig --engine-version ENGINE_VERSION --root-public-key /root/trusted-root-public.pem --nginx-site /etc/nginx/sites-available/shop
```

After reviewing preview, add `--apply` to install infrastructure, briefly test
public 503/private probe and enable timer/path without deploying application/SQL.
Retain private backups of replaced files. Full operational guide:
`docs/guides/application-updates.md` at the recorded source tag.
