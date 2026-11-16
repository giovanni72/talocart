// Whole-database SMTP recovery on disposable local PostgreSQL databases only.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
(async () => {
  const sourceURL = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  assert.ok(['localhost', '127.0.0.1', '[::1]'].includes(sourceURL.hostname) && sourceURL.pathname.endsWith('_test') && !sourceURL.search, 'Dedicated local _test database required');
  const binary = path.resolve(process.env.TALOCART_BASE_TEST_BINARY || 'target/debug/talocart');
  const operations = path.resolve('scripts/release/operations.py');
  const python = process.env.PYTHON || 'python3';
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-smtp-recovery-')));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('TALOCART_') && !key.startsWith('PG')));
  const owner = new Client({ connectionString: sourceURL.href });
  const created = [];
  let child;
  const run = (exe, args, expected = 0) => {
    const result = spawnSync(exe, args, { env, cwd: root, encoding: 'utf8', timeout: 180000 });
    assert.equal(result.status, expected, 'Recovery command failed; private diagnostics omitted');
  };
  if (process.env.RELEASE_TEST_PG_CONTAINER) {
    const clients = path.join(root, 'clients'); fs.mkdirSync(clients);
    const adapter = path.resolve('scripts/release/pg-container.py');
    const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
    for (const tool of ['psql', 'pg_dump', 'pg_restore']) {
      fs.writeFileSync(path.join(clients, tool), `#!/bin/sh\nexec ${quote(python)} ${quote(adapter)} ${tool} "$@"\n`, { mode: 0o700 });
    }
    env.PATH = clients + path.delimiter + process.env.PATH;
  }
  const stop = async () => { if (child && child.exitCode === null) { const done = once(child, 'exit'); child.kill('SIGTERM'); await done; } };
  const origin = 'http://127.0.0.1:3168';
  const start = async target => {
    child = spawn(binary, ['--config', target.config, 'serve'], { env, cwd: root, stdio: 'ignore' });
    for (let i = 0; i < 150; i++) {
      assert.equal(child.exitCode, null, 'Recovered host exited');
      try { if ((await fetch(origin + '/health/ready')).ok) return; } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw Error('Recovery host readiness timeout');
  };
  const get = async (route, cookie) => {
    const response = await fetch(origin + route, { headers: cookie ? { cookie } : {}, redirect: 'manual' });
    return { response, html: await response.text() };
  };
  const csrf = html => { const token = html.match(/name="csrf" value="([^"]+)"/)?.[1]; assert.ok(token); return token; };
  const cookie = response => response.headers.getSetCookie().find(v => v.startsWith('talocart_session='))?.split(';')[0];
  const post = (route, session, fields) => fetch(origin + route, { method: 'POST', redirect: 'manual', headers: { cookie: session || '', 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(fields) });
  try {
    await owner.connect();
    const targets = [];
    for (const label of ['source', 'restored']) {
      const name = `talocart_smtp_${label}_${process.pid}_${randomBytes(4).toString('hex')}_test`;
      await owner.query(`CREATE DATABASE ${name}`); created.push(name);
      const url = new URL(sourceURL); url.pathname = '/' + name;
      const folder = path.join(root, label); fs.mkdirSync(folder);
      const config = path.join(folder, 'talocart.toml');
      fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:3168"\n[database]\nurl=${JSON.stringify(url.href)}\n[media]\nroot="media"\n[packages]\nroot="packages"\n[logging]\nroot="logs"\n`, { mode: 0o600 });
      targets.push({ folder, config, url: url.href });
    }
    const [source, restored] = targets;
    run(binary, ['--config', source.config, 'migrate']);
    fs.mkdirSync(path.join(source.folder, 'media')); fs.writeFileSync(path.join(source.folder, 'media/fixture'), 'synthetic media');
    await start(source);
    let page = await get('/setup');
    const password = randomBytes(24).toString('hex');
    let response = await post('/setup', cookie(page.response), { csrf: csrf(page.html), email: 'owner@example.test', password, name: 'SMTP recovery', display_name: 'Owner', primary_locale: 'en', locale: 'en' });
    assert.equal(response.status, 303);
    page = await get('/admin/login');
    response = await post('/admin/login', cookie(page.response), { csrf: csrf(page.html), email: 'owner@example.test', password });
    assert.equal(response.status, 303); const session = cookie(response);
    page = await get('/admin/settings/email', session);
    response = await post('/admin/settings/email/transport', session, { csrf: csrf(page.html), revision: '0', host: 'smtp.example.test', port: '587', security: 'starttls', username: 'synthetic-user', password: 'synthetic-recovery-secret', timeout_seconds: '10' });
    assert.equal(response.status, 303);
    await stop();
    const key = path.join(source.folder, 'media.secrets/smtp.key');
    const expectedKey = fs.readFileSync(key); assert.equal(expectedKey.length, 32);
    const backup = path.join(root, 'backup');
    run(python, [operations, 'backup', '--config', source.config, '--binary', binary, '--output', backup]);
    assert.equal(JSON.parse(fs.readFileSync(path.join(backup, 'backup.json'))).smtp_secrets_included, true);
    run(python, [operations, 'restore', '--backup', backup, '--config', restored.config]);
    const restoredKey = path.join(restored.folder, 'media.secrets/smtp.key');
    assert.deepEqual(fs.readFileSync(restoredKey), expectedKey);
    run(binary, ['--config', restored.config, 'doctor']);
    await start(restored);
    page = await get('/admin/settings/email', session);
    assert.equal(page.response.status, 200); assert.ok(page.html.includes('Password saved.')); assert.ok(!page.html.includes('synthetic-recovery-secret'));
    await stop();
    fs.renameSync(restoredKey, restoredKey + '.saved');
    run(binary, ['--config', restored.config, 'doctor'], 1);
    run(binary, ['--config', restored.config, 'serve'], 1);
    assert.equal(fs.existsSync(restoredKey), false, 'Lost key must never be regenerated');
    fs.writeFileSync(restoredKey, randomBytes(32), { mode: 0o600 });
    run(binary, ['--config', restored.config, 'doctor'], 1);
    run(binary, ['--config', restored.config, 'serve'], 1);
    fs.unlinkSync(restoredKey); fs.renameSync(restoredKey + '.saved', restoredKey);
    run(binary, ['--config', restored.config, 'doctor']);
    console.log('SMTP recovery: encrypted credentials, private key, restored host, missing/wrong-key refusal passed');
  } finally {
    await stop();
    for (const name of created.reverse()) await owner.query(`DROP DATABASE IF EXISTS ${name} WITH (FORCE)`);
    await owner.end(); fs.rmSync(root, { recursive: true, force: true });
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
