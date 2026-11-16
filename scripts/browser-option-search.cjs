// Isolated synthetic catalog: option value search in every active language, EN/IT, desktop/mobile, JS/no-JS.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  await db.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const schema = `talocart_option_search_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-option-search-'));
  const origin = 'http://127.0.0.1:3147';
  const output = path.resolve('.local/verification/option-search');
  fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const config = path.join(folder, 'talocart.toml');
  fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3147"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3147', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
  let server;
  const results = [];
  try {
    await db.query(`CREATE SCHEMA ${schema}`);
    await db.query(`SET search_path TO ${schema}`);
    assert.equal(spawnSync(binary, ['--config', config, 'migrate'], { env, stdio: 'pipe' }).status, 0, 'Clean migration');
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: 'ignore' });
    let ready = false;
    for (let i = 0; i < 150; i++) {
      try { if ((await fetch(origin + '/health/ready')).ok) { ready = true; break; } } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(ready, 'Synthetic server readiness');
    const setup = await browser.newContext();
    const admin = await setup.newPage();
    const password = randomBytes(20).toString('hex');
    await setup.addCookies([{ name: 'talocart_admin_language', value: 'it', url: origin }]);
    await admin.goto(origin + '/setup');
    await admin.getByLabel('Nome del negozio', { exact: true }).fill('Option search');
    await admin.getByLabel('Email amministratore').fill('owner@example.test');
    await admin.getByLabel('Password', { exact: true }).fill(password);
    await admin.getByRole('button', { name: 'Crea negozio', exact: true }).click();
    await admin.waitForURL('**/admin/login');
    await admin.getByLabel('Email amministratore').fill('owner@example.test');
    await admin.getByLabel('Password', { exact: true }).fill(password);
    await admin.getByRole('button', { name: 'Accedi', exact: true }).click();
    await admin.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(cookie => cookie.name === 'talocart_session');
    await setup.close();
    const option = (await db.query("INSERT INTO catalog_options(name) VALUES('Colore') RETURNING id")).rows[0].id;
    await db.query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Tono '||lpad(n::text,2,'0') FROM generate_series(1,60)n", [option]);
    const green = (await db.query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Verde bosco') RETURNING id", [option])).rows[0].id;
    await db.query("INSERT INTO catalog_option_values_translations(entity_id,locale,name,option_id) VALUES($1,'en','Forest green',$2)", [green, option]);
    const editor = `${origin}/admin/catalog/options/${option}`;
    for (const locale of ['it', 'en']) for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      await db.query('UPDATE administrators SET locale=$1', [locale]);
      const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js });
      await context.addCookies(cookies);
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      const t = (it, en) => locale === 'it' ? it : en;
      const rows = () => page.locator('.option-value-row').count();
      try {
        await page.goto(editor);
        assert.equal(await page.locator('html').getAttribute('lang'), locale);
        const field = page.getByRole('searchbox', { name: t('Cerca valori', 'Search values'), exact: true });
        assert.equal(await field.getAttribute('aria-describedby'), 'option-values-q-help');
        // Keyboard: type the English name and submit natively with Enter.
        await field.focus();
        await page.keyboard.type('FOREST');
        await Promise.all([page.waitForURL(u => new URL(u).searchParams.get('q') === 'FOREST'), page.keyboard.press('Enter')]);
        assert.equal(await rows(), 1);
        assert.equal(await page.locator(`#value-${green}`).inputValue(), 'Verde bosco');
        assert.equal(await page.locator('.option-values-results').innerText(), t('1 valore corrisponde a «FOREST».', '1 value matches “FOREST”.'));
        assert.equal(await field.inputValue(), 'FOREST');
        // Saving inside the search keeps it, with the save button reached by keyboard.
        await page.locator(`#value-${green}`).fill('Verde bosco scuro');
        const save = page.locator(`form[action$="/values/${green}"]`).getByRole('button', { name: t('Salva valore', 'Save value'), exact: true });
        await save.focus();
        await Promise.all([page.waitForURL(u => new URL(u).searchParams.get('saved') === '1'), page.keyboard.press('Enter')]);
        assert.equal(new URL(page.url()).searchParams.get('q'), 'FOREST');
        assert.equal(await rows(), 1);
        assert.equal(await page.locator(`#value-${green}`).inputValue(), 'Verde bosco scuro');
        // Primary-language search spans pages and keeps the search in pagination links.
        await field.fill('tono');
        await Promise.all([page.waitForURL(u => new URL(u).searchParams.get('q') === 'tono'), field.press('Enter')]);
        assert.equal(await rows(), 50);
        await Promise.all([page.waitForURL(u => new URL(u).searchParams.get('page') === '2'), page.getByRole('link', { name: t('Successivo', 'Next') }).click()]);
        assert.equal(new URL(page.url()).searchParams.get('q'), 'tono');
        assert.equal(await rows(), 10);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await page.screenshot({ path: path.join(output, `${locale}-${profile}-results.png`), fullPage: true });
        // No match: dedicated empty state; clearing restores the full list.
        await field.fill('assente');
        await Promise.all([page.waitForURL(u => new URL(u).searchParams.get('q') === 'assente'), field.press('Enter')]);
        assert.equal(await rows(), 0);
        assert.ok((await page.locator('.ui-empty').innerText()).includes(t('Nessun valore corrisponde alla ricerca.', 'No values match this search.')));
        const clear = page.getByRole('link', { name: t('Azzera ricerca', 'Clear search'), exact: true }).first();
        await Promise.all([page.waitForURL(u => !new URL(u).searchParams.has('q')), clear.click()]);
        assert.equal(await rows(), 50);
        assert.equal(await field.inputValue(), '');
        // Invalid text is reported on the field and not applied.
        const response = await page.goto(`${editor}?q=${'x'.repeat(81)}`);
        assert.equal(response.status(), 422);
        assert.equal(await field.getAttribute('aria-invalid'), 'true');
        assert.equal(await field.getAttribute('aria-describedby'), 'option-values-q-help option-values-q-error');
        assert.equal(await page.locator('#option-values-q-error').innerText(), t('Inserisci al massimo 80 caratteri senza caratteri di controllo.', 'Enter up to 80 characters without control characters.'));
        assert.equal(await rows(), 50);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await page.screenshot({ path: path.join(output, `${locale}-${profile}-invalid.png`) });
        // Restore the primary name (base row and its primary-language translation) for the next profile.
        await db.query("UPDATE catalog_option_values SET name='Verde bosco' WHERE id=$1", [green]);
        await db.query("UPDATE catalog_option_values_translations SET name='Verde bosco' WHERE entity_id=$1 AND locale='it'", [green]);
        assert.deepEqual(errors, []);
        results.push({ locale, profile, passed: true });
        console.log(locale, profile, 'passed');
      } finally { await context.close(); }
    }
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2));
  } finally {
    if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; }
    await browser.close();
    await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    await db.end();
    fs.rmSync(folder, { recursive: true, force: true });
  }
})().catch(error => { console.error(error.stack); process.exitCode = 1; });
