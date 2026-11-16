const presentation = require('./theme-presentation-helper.cjs');
// Development-only E2E. A unique PostgreSQL schema is removed in finally.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
const uploadLibrary = require('./media-picker-helper.cjs');

function settingsMenu(page) {
  return page.getByRole('navigation', { name: 'Amministrazione', exact: true }).locator('.sidebar-group').filter({ has: page.locator('summary', { hasText: /^Impostazioni$/ }) });
}
async function openGeneralSettings(page) {
  const menu = settingsMenu(page);
  if (!(await menu.evaluate(el => el.open))) { await menu.locator('summary').focus(); await page.keyboard.press('Enter'); }
  await menu.getByRole('link', { name: 'Generali', exact: true }).click();
  await page.waitForURL('**/admin/settings');
}
async function productSection(page, name) {
  const tab = page.getByRole('tab', { name, exact: true });
  if (await tab.count()) await tab.click();
  else await page.locator('.product-tab-editor .ui-tabs').getByRole('link', { name, exact: true }).click();
}
async function assertCompactTitle(page, allowWrap = false) {
  await require('./ui-contract.cjs')(page);
  const title = page.locator('h1');
  if (await title.count()) {
    const metrics = await title.evaluate(el => ({ breaks: el.querySelectorAll('br').length, size: parseFloat(getComputedStyle(el).fontSize), height: el.getBoundingClientRect().height, lineHeight: parseFloat(getComputedStyle(el).lineHeight) }));
    assert.equal(metrics.breaks, 0, 'Page title has no forced line breaks');
    assert.ok(metrics.size <= 32, 'Page title is compact');
    if (!allowWrap) assert.ok(metrics.height <= metrics.lineHeight + 1, 'Page title fits on one line');
    else assert.ok(await title.evaluate(el => el.scrollWidth <= el.getBoundingClientRect().width + 1), 'Search text wraps within the heading');
  }
}

async function assertAdminBrandSurface(page) {
  const colors = await page.locator('.admin-sidebar .wordmark').evaluate(link => ({
    background: getComputedStyle(link).backgroundColor,
    text: getComputedStyle(link).color,
    pageBackground: getComputedStyle(document.body).backgroundColor,
    pageText: getComputedStyle(document.body).color,
  }));
  assert.equal(colors.background, colors.pageBackground, 'Backend brand uses the page background');
  assert.equal(colors.text, colors.pageText, 'Store name remains readable on the light background');
}

(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Set TALOCART_TEST_DATABASE_URL to a dedicated test database');
  const owner = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL });
  await owner.connect();
  const schema = `talocart_browser_${process.pid}_${Date.now()}`;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-browser-'));
  const output = path.resolve('.local/verification/product-variants');
  fs.mkdirSync(output, { recursive: true });
  const port = Number(process.env.TALOCART_TEST_PORT || 3101);
  assert.ok(Number.isInteger(port) && port > 1024 && port < 65536);
  const origin = `http://127.0.0.1:${port}`;
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const config = path.join(temp, 'talocart.toml');
  fs.writeFileSync(config, `[server]\nlisten="127.0.0.1:${port}"\n`, { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: `127.0.0.1:${port}`, TALOCART_STORE_NAME: 'Koala', TALOCART_LOG: 'warn', TALOCART_MEDIA_ROOT: path.join(temp, 'media') };
  const cli = (...args) => {
    const result = spawnSync(binary, ['--config', config, ...args], { env, encoding: 'utf8' });
    assert.equal(result.status, 0, `CLI ${args[0]} failed (output omitted to protect tokens)`);
    return result;
  };
  const password = randomBytes(20).toString('hex');
  const email = 'browser@example.test';
  let server, browser;
  const results = [];
  const start = async () => {
    server = spawn(binary, ['--config', config, 'serve'], { env, stdio: 'ignore' });
    for (let i = 0; i < 100; i++) {
      if (server.exitCode !== null) throw new Error('Test server exited');
      try { const r = await fetch(origin + '/health/ready'); if (r.ok) return; } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error('Test server readiness timeout');
  };
  const stop = async () => {
    if (server && server.exitCode === null) {
      const done = once(server, 'exit'); server.kill('SIGTERM');
      const [code] = await done; assert.equal(code, 0, 'Graceful shutdown');
    }
  };
  // Leave the page before a deliberate outage so late asset requests do not race shutdown.
  const restartPage = async page => {
    const returnTo = page.url();
    await page.goto('about:blank'); await stop(); await start(); await page.goto(returnTo);
  };
  try {
    await owner.query(`CREATE SCHEMA ${schema}`);
    cli('migrate'); cli('migrate'); cli('doctor');
    await start();
    browser = await chromium.launch({ headless: true, channel: process.env.TALOCART_BROWSER_CHANNEL || undefined });
    for (const [name, viewport, js] of [
      ['desktop', { width: 1440, height: 1050 }, true],
      ['mobile', { width: 390, height: 844 }, true],
      ['no-js', { width: 1440, height: 1050 }, false],
    ]) {
      const context = await browser.newContext({ viewport, javaScriptEnabled: js });
      context.on('page', p => p.on('dialog', d => d.accept()));
      await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
      const setupPage = await context.newPage();
      await setupPage.goto(origin + '/setup');
      await require('./ui-contract.cjs')(setupPage);
      assert.equal(await setupPage.locator('form[action="/setup"] input[name="token"]').count(),0);
      assert.equal(await setupPage.getByLabel('Installa dati demo', { exact: true }).isChecked(), false);
      assert.equal(await setupPage.getByLabel('Installa dati demo', { exact: true }).isEnabled(), true);
      const displayName = setupPage.getByLabel('Nome visualizzato', { exact: true });
      assert.equal(await displayName.getAttribute('maxlength'), '80');
      assert.equal(await displayName.getAttribute('autocomplete'), 'name');
      assert.equal(await displayName.getAttribute('required'), null);
      await setupPage.getByLabel('Nome del negozio').focus();
      await setupPage.keyboard.press('Tab');
      assert.equal(await displayName.evaluate(input => input === document.activeElement), true);
      assert.ok(await setupPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      { const shot = { path: path.join(output, `setup-${name}.png`), fullPage: true }; await presentation.verify(setupPage, path.basename(shot.path)); await presentation.screenshot(setupPage, shot); }
      await context.close();
    }
    const setup = await browser.newContext({ viewport: { width: 390, height: 844 }, javaScriptEnabled: false });
    await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
    const page = await setup.newPage();
    await page.goto(origin + '/admin');
    await page.waitForURL('**/setup');
    await page.getByLabel('Nome del negozio').fill('Talocart Atelier');
    await page.getByLabel('Nome visualizzato', { exact: true }).fill('  Primo amministratore  ');
    await page.getByLabel('Email amministratore').fill(email);
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByRole('button', { name: 'Crea negozio' }).click();
    await page.waitForURL('**/admin/login');
    const initialProfile = await owner.query(`SELECT display_name FROM ${schema}.administrators WHERE email=$1`, [email]);
    assert.equal(initialProfile.rows[0].display_name, 'Primo amministratore');
    assert.equal(Number((await owner.query(`SELECT count(*) FROM ${schema}.catalog_demo_packages`)).rows[0].count), 0);
    await stop(); await start();
    await setup.close(); results.push('Bootstrap with display name: desktop/mobile/keyboard/no-JS, persistence after restart, no installation code, repeated migrations: OK');
    for (const [name, viewport, js] of [
      ['desktop', { width: 1440, height: 1050 }, true],
      ['mobile', { width: 390, height: 844 }, true],
      ['no-js', { width: 1440, height: 1050 }, false],
    ]) {
      const context = await browser.newContext({ viewport, javaScriptEnabled: js });
      context.on('page', p => p.on('dialog', d => d.accept()));
      const page = await context.newPage(); const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
      await page.goto(origin + '/admin'); await page.waitForURL('**/admin/login');
      await page.getByLabel('Lingua di accesso', { exact: true }).selectOption('en');
      await page.getByRole('button', { name: 'Applica', exact: true }).click();
      await page.waitForURL('**/admin/login');
      assert.equal(await page.locator('html').getAttribute('lang'), 'en');
      await page.getByRole('link', { name: 'Forgot your password?', exact: true }).click();
      await page.waitForURL('**/admin/reset');
      assert.equal(await page.locator('html').getAttribute('lang'), 'en');
      await page.getByLabel('Sign-in language', { exact: true }).selectOption('it');
      await page.getByRole('button', { name: 'Apply', exact: true }).click();
      await page.waitForURL('**/admin/reset');
      await page.getByRole('link', { name: 'Torna all’accesso', exact: true }).click();
      await page.waitForURL('**/admin/login');
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `login-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.getByLabel('Email amministratore').fill(email);
      await page.getByLabel('Password', { exact: true }).fill(password);
      await page.getByRole('button', { name: 'Accedi' }).click(); await page.waitForURL('**/admin');
      await page.goto(origin + '/admin/account');
      await page.locator('.admin-sidebar .wordmark').click();
      await page.waitForLoadState('networkidle');
      assert.equal(new URL(page.url()).pathname, '/admin', 'Backend brand returns to overview');
      await assertAdminBrandSurface(page);
      if (name === 'desktop') assert.equal(await page.locator('.account-menu-name').innerText(), 'Primo amministratore');
      const accountMenu = page.locator('.account-menu');
      assert.equal(await page.locator('.admin-topbar .phase-badge').count(), 0);
      await page.getByLabel('Menu account', { exact: true }).focus();
      await page.keyboard.press('Enter');
      await accountMenu.getByRole('link', { name: 'Il mio account', exact: true }).click();
      await page.waitForURL('**/admin/account');
      await page.getByLabel('Nome visualizzato', { exact: true }).fill(`Persona ${name}`);
      await page.getByRole('button', { name: 'Salva profilo', exact: true }).click();
      await page.waitForURL('**/admin/account?saved=1');
      assert.equal(await page.getByLabel('Nome visualizzato', { exact: true }).inputValue(), `Persona ${name}`);
      assert.equal(await uploadLibrary(page, 'Seleziona immagine personale', 'crates/web/tests/fixtures/avatar.png'), 303);
      await page.waitForURL('**/admin/account?saved=1');
      const accountImage = page.locator('.account-avatar-large img');
      await accountImage.waitFor();
      assert.ok(await accountImage.evaluate(img => img.complete && img.naturalWidth === 128));
      assert.equal(await page.locator('input[type=file]').count(), 0);
      assert.equal(await uploadLibrary(page, 'Seleziona immagine personale', 'crates/web/tests/fixtures/webp-lossy.webp'), 303);
      await page.waitForURL('**/admin/account?saved=1');
      assert.ok(await accountImage.evaluate(img => img.complete && img.naturalWidth === 128));
      assert.equal((await context.request.get(origin + '/admin/account/avatar')).headers()['content-type'], 'image/png');
      results.push(`${name}: WebP avatar upload normalized to PNG: OK`);
      if (name === 'desktop') {
        await restartPage(page);
        assert.equal(await page.getByLabel('Nome visualizzato', { exact: true }).inputValue(), `Persona ${name}`);
        assert.ok(await accountImage.evaluate(img => img.complete && img.naturalWidth === 128));
      }
      await page.getByLabel('Menu account', { exact: true }).click();
      assert.ok(await accountMenu.getByRole('button', { name: 'Esci', exact: true }).isVisible());
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `account-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.getByLabel('Menu account', { exact: true }).click();
      await page.getByRole('button', { name: 'Rimuovi immagine', exact: true }).click();
      await page.waitForURL('**/admin/account?saved=1');
      assert.equal(await page.locator('.account-avatar img').count(), 0);
      results.push(`${name}: personal profile, account menu by keyboard, upload/removal and persistence OK`);
      await require('./browser-localization.cjs')({ page, context, origin, name, js, output, restartPage });
      results.push(`${name}: Italian/English preference, all administration areas, validation, status, keyboard, 320–2560px, no-JS and restart OK`);
      await openGeneralSettings(page);
      await page.waitForURL('**/admin/settings');
      assert.ok(await settingsMenu(page).getByRole('link', { name: 'Generali', exact: true }).evaluate(el => el.getAttribute('aria-current') === 'page'));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      const settingsValues = [
        ['Nome del negozio', 'Atelier persistente'], ['Ragione sociale', 'Atelier Esempio Srl'],
        ['Email di contatto', 'contatti@example.test'], ['Telefono', '+39 02 1234567'],
        ['Indirizzo', 'Via dei Tigli 12'], ['Complemento indirizzo', 'Scala B, interno 4'],
        ['CAP', '20100'], ['Città', 'Milano'], ['Provincia / Regione', 'MI'], ['Paese', 'Italia'],
      ];
      for (const [label, value] of settingsValues) {
        await page.getByRole('textbox', { name: label, exact: true }).fill(value);
      }
      await page.getByRole('button', { name: 'Salva impostazioni' }).click();
      await page.waitForURL('**/admin/settings?saved=1');
      for (const [label, value] of settingsValues) assert.equal(await page.getByRole('textbox', { name: label, exact: true }).inputValue(), value);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `settings-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      if (name === 'desktop') {
        await restartPage(page);
        for (const [label, value] of settingsValues) assert.equal(await page.getByRole('textbox', { name: label, exact: true }).inputValue(), value);
        assert.equal(new URL(page.url()).pathname, '/admin/settings');
        results.push('Real process restart preserves authenticated session and all general settings: OK');
      }
      await page.locator('#settings-tab-appearance').click();
      assert.ok(await page.locator('#appearance').isVisible());
      for (const key of ['logo', 'favicon']) {
        assert.equal(await uploadLibrary(page, `Seleziona ${key}`, `crates/web/tests/fixtures/${key === 'logo' ? 'logo' : 'avatar'}.png`), 303);
        await page.waitForURL('**/admin/settings/appearance?saved=1');
        const image = page.locator(`.branding-preview-${key} img`);
        assert.ok(await image.evaluate(img => img.complete && img.naturalWidth > 0));
        const response = await context.request.get(origin + '/branding/' + key);
        assert.equal(response.headers()['content-type'], 'image/png');
        assert.equal(response.headers()['cache-control'], 'public, max-age=0, must-revalidate');
        const metadata = await owner.query(`SELECT s.${key}_key AS key,m.byte_size FROM ${schema}.store_settings s JOIN ${schema}.media_objects m ON m.key=s.${key}_key`);
        const file = path.join(temp, 'media', 'objects', metadata.rows[0].key);
        assert.deepEqual(fs.readFileSync(file), await response.body());
        assert.equal(fs.statSync(file).size, Number(metadata.rows[0].byte_size));
        const before = metadata.rows[0].key;
        assert.match(before, /-(logo|favicon)(?:-\d+)?\.png$/);
        const cached = await context.request.get(response.url(), { headers: { 'If-None-Match': response.headers().etag } });
        assert.equal(cached.status(), 304);
        assert.equal((await cached.body()).length, 0);
        if (name === 'desktop' && key === 'logo') {
          const invalidPage = await context.newPage();
          await invalidPage.goto(origin + '/admin/settings/appearance');
          assert.equal(await uploadLibrary(invalidPage, 'Seleziona logo', { name: 'fake.png', mimeType: 'image/png', buffer: Buffer.from('<svg onload="alert(1)"/>') }), 422);
          assert.equal(await invalidPage.getByLabel('File da caricare', { exact: true }).getAttribute('aria-invalid'), 'true');
          assert.equal((await owner.query(`SELECT logo_key FROM ${schema}.store_settings`)).rows[0].logo_key, before);
          { const shot = { path: path.join(output, 'branding-error.png'), fullPage: true }; await presentation.verify(invalidPage, path.basename(shot.path)); await presentation.screenshot(invalidPage, shot); }
          await invalidPage.close();
        }
        assert.equal(await uploadLibrary(page, `Seleziona ${key}`, `crates/web/tests/fixtures/${key === 'logo' ? 'webp-lossless' : 'webp-lossy'}.webp`), 303);
        await page.waitForURL('**/admin/settings/appearance?saved=1');
        assert.notEqual((await owner.query(`SELECT ${key}_key AS key FROM ${schema}.store_settings`)).rows[0].key, before);
      }
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `branding-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      if (name === 'desktop') {
        await restartPage(page);
        assert.equal(await page.getByRole('button', { name: 'Rimuovi logo', exact: true }).count(), 1);
        assert.equal((await context.request.get(origin + '/branding/favicon')).headers()['content-type'], 'image/png');
        const logo = (await owner.query(`SELECT logo_key FROM ${schema}.store_settings`)).rows[0].logo_key;
        const logoPath = path.join(temp, 'media', 'objects', logo);
        const savedLogo = fs.readFileSync(logoPath);
        fs.unlinkSync(logoPath);
        await page.goto('about:blank'); await stop(); await start();
        const missing = await context.request.get(origin + '/branding/logo');
        assert.equal(missing.status(), 200);
        assert.equal(missing.headers()['content-type'], 'image/svg+xml');
        assert.equal(missing.headers()['cache-control'], 'no-store');
        fs.writeFileSync(logoPath, savedLogo);
        results.push('Missing image does not block process startup; fallback is not cached: OK');
      }
      for (const route of ['/', '/admin', '/admin/login']) {
        if (route === '/admin/login') {
          const anonymous = await browser.newContext({ viewport, javaScriptEnabled: js });
          const loginPage = await anonymous.newPage(); await loginPage.goto(origin + route);
          assert.ok(await loginPage.locator('.brand-logo').evaluate(img => img.complete && img.naturalWidth > 0));
          assert.equal(await loginPage.locator('link[rel=icon]').getAttribute('href'), '/branding/favicon');
          await anonymous.close();
        } else {
          await page.goto(origin + route);
          assert.ok(await page.locator('.brand-logo').evaluateAll(images => images.length > 0 && images.every(img => img.complete && img.naturalWidth > 0)));
          assert.equal(await page.locator('link[rel=icon]').getAttribute('href'), '/branding/favicon');
          assert.ok((await page.locator(route === '/' ? '.site-header .wordmark' : '.admin-sidebar .wordmark').allTextContents()).every(text=>text.trim()===''));
          const shapes = await page.locator('.brand-logo').evaluateAll(images => images.map(img => ({ natural: img.naturalWidth / img.naturalHeight, rendered: img.getBoundingClientRect().width / img.getBoundingClientRect().height })));
          for(const shape of shapes){assert.equal(shape.natural, 4);assert.ok(Math.abs(shape.rendered - 4) < 0.05, 'Every rectangular logo keeps its proportions');}
          if (route === '/admin') await assertAdminBrandSurface(page);
          assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
          await assertCompactTitle(page); { const shot = { path: path.join(output, `logo-${route === '/' ? 'store' : 'admin'}-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        }
      }
      await page.goto(origin + '/admin/settings/appearance');
      for (const key of ['logo', 'favicon']) {
        await page.getByRole('button', { name: `Rimuovi ${key}`, exact: true }).click();
        await page.locator('#settings-save').click();
        await page.waitForURL('**/admin/settings/appearance?saved=1');
        assert.equal((await context.request.get(origin + '/branding/' + key)).headers()['content-type'], 'image/svg+xml');
        assert.equal(await page.getByRole('button', { name: `Seleziona ${key}`, exact: true }).count(), 1);
      }
      for (const route of ['/', '/admin']) {
        await page.goto(origin + route);
        assert.equal(await page.locator('.wordmark img').count(), 0);
        assert.ok((await page.locator(route === '/' ? '.site-header .wordmark' : '.admin-sidebar .wordmark').allTextContents()).every(text=>text.trim()==='Atelier persistente'));
        if (route === '/admin') {
          await assertAdminBrandSurface(page);
          await assertCompactTitle(page); { const shot = { path: path.join(output, `brand-name-admin-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        }
      }
      await page.goto(origin + '/admin/settings/appearance');
      await page.locator('#settings-tab-general').click();
      for (const [label, value] of settingsValues) assert.equal(await page.getByRole('textbox', { name: label, exact: true }).inputValue(), value);
      results.push(`${name}: filesystem branding upload, invalid content, replacement, removal, store/admin/login and restart OK`);
      results.push(`${name}: backend brand uses page background with rectangular logo or readable store name OK`);
      await page.locator('.admin-sidebar .wordmark').click();
      await page.waitForURL('**/admin');
      results.push(`${name}: general settings and backend brand navigation OK`);
      await page.goto(origin + '/admin');
      const catalogueSummary=page.getByRole('navigation', { name: 'Amministrazione', exact: true }).locator('summary', {hasText:/^Catalogo$/});
      await catalogueSummary.focus();
      assert.ok(await catalogueSummary.evaluate(e=>document.activeElement===e),'Catalogue group receives keyboard focus');
      await page.keyboard.press('Enter');
      await page.getByRole('link', { name: 'Categorie', exact: true }).click();
      await page.waitForURL('**/admin/catalog/categories');
      assert.equal(await page.getByRole('link', { name: 'Categorie', exact: true }).getAttribute('aria-current'), 'page');
      await page.getByRole('link', { name: 'Nuova categoria', exact: false }).click();
      await page.getByLabel('Nome della categoria', { exact: true }).fill(`Abiti ${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`abiti-${name}`);
      await page.getByLabel('Descrizione', { exact: true }).fill('Testo semplice <b>senza HTML</b>');
      await page.getByLabel('Ordine di visualizzazione', { exact: true }).fill('10');
      await page.getByRole('button', { name: 'Crea categoria', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/categories\/\d+\?saved=1$/);
      const rootCategoryPath = new URL(page.url()).pathname;
      const rootCategoryId = rootCategoryPath.split('/').at(-1);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `category-edit-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.goto(origin + '/admin/catalog/categories/new');
      await page.getByLabel('Nome della categoria', { exact: true }).fill(`Uomo ${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`uomo-${name}`);
      await page.getByLabel('Categoria superiore', { exact: true }).selectOption(rootCategoryId);
      await page.getByRole('button', { name: 'Crea categoria', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/categories\/\d+\?saved=1$/);
      const childCategoryPath = new URL(page.url()).pathname;
      assert.equal(await page.getByLabel('Categoria superiore', { exact: true }).inputValue(), rootCategoryId);
      await page.getByLabel('Categoria attiva', { exact: true }).uncheck();
      await page.getByRole('button', { name: 'Salva categoria', exact: true }).click();
      await page.waitForURL('**' + childCategoryPath + '?saved=1');
      const blocked = await context.newPage();
      await blocked.goto(origin + rootCategoryPath);
      assert.equal(await blocked.getByLabel('Categoria superiore', { exact: true }).locator(`option[value="${childCategoryPath.split('/').at(-1)}"]`).count(), 0);
      await blocked.getByLabel('Confermo l’eliminazione', { exact: true }).check();
      const deniedDelete = blocked.waitForResponse(r => r.url().endsWith(rootCategoryPath + '/delete') && r.request().method() === 'POST');
      await blocked.getByRole('button', { name: 'Elimina categoria', exact: true }).click();
      assert.equal((await deniedDelete).status(), 409);
      await blocked.getByRole('alert').waitFor();
      assert.ok(await blocked.getByRole('alert').innerText().then(text => text.includes('Sposta prima')));
      await blocked.close();
      await page.goto(origin + '/admin/catalog/categories');
      await page.getByLabel('Cerca per nome o slug', { exact: true }).fill(`uomo-${name}`);
      await page.getByLabel('Stato', { exact: true }).selectOption('inactive');
      await page.getByRole('button', { name: 'Filtra', exact: true }).click();
      await page.waitForURL('**/admin/catalog/categories?**');
      const filteredCategory = page.locator('.category-tree-node[data-context="false"] > .category-node-card');
      assert.equal(await filteredCategory.count(), 1);
      assert.ok(await filteredCategory.innerText().then(text => text.includes(`Uomo ${name}`) && text.includes(`Abiti ${name}`) && text.includes('Disattivato')));
      const categoryContext = page.locator('.category-tree-node[data-context="true"] > .category-node-card');
      assert.equal(await categoryContext.count(), 1);
      assert.ok(await categoryContext.innerText().then(text => text.includes(`Abiti ${name}`) && text.includes('Contesto')));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `categories-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.goto(origin + '/admin/catalog/brands/new');
      await page.getByLabel('Nome del marchio', { exact: true }).fill(`Marchio ${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`marchio-${name}`);
      await page.getByRole('button', { name: 'Crea marchio', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/brands\/\d+\?saved=1$/);
      const brandPath = new URL(page.url()).pathname;
      assert.equal(await uploadLibrary(page, 'Seleziona logo del marchio', 'crates/web/tests/fixtures/webp-lossless.webp'), 303);
      await page.waitForURL('**' + brandPath + '?saved=1');
      assert.ok(await page.locator('.catalog-logo-preview img').evaluate(img => img.complete && img.naturalWidth > 0));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `brand-edit-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const duplicate = await context.newPage(); await duplicate.goto(origin + '/admin/catalog/brands/new');
      await duplicate.getByLabel('Nome del marchio', { exact: true }).fill(`Marchio ${name}`);
      await duplicate.getByLabel('Slug', { exact: true }).fill(`duplicato-${name}`);
      const duplicateResponse = duplicate.waitForResponse(r => r.url().endsWith('/admin/catalog/brands/new') && r.request().method() === 'POST');
      await duplicate.getByRole('button', { name: 'Crea marchio', exact: true }).click();
      assert.equal((await duplicateResponse).status(), 409);
      assert.equal(await duplicate.getByLabel('Nome del marchio', { exact: true }).getAttribute('aria-invalid'), 'true');
      await duplicate.goto(origin + brandPath);
      assert.equal(await uploadLibrary(duplicate, 'Seleziona logo del marchio', { name: 'invalid.webp', mimeType: 'image/webp', buffer: Buffer.from('invalid') }), 422);
      assert.ok((await duplicate.getByRole('alert').innerText()).includes('WebP'));
      await duplicate.goto(origin + brandPath);
      assert.equal(await duplicate.locator('.catalog-logo-preview img').count(), 1);
      await duplicate.close();
      if (name === 'desktop') { await restartPage(page); await page.goto(origin + brandPath); assert.equal(await page.locator('.catalog-logo-preview img').count(), 1); }
      await page.getByRole('button', { name: 'Rimuovi logo del marchio', exact: true }).click();
      await page.waitForURL('**' + brandPath + '?saved=1');
      assert.equal(await page.locator('.catalog-logo-preview img').count(), 0);
      await page.goto(origin + '/admin/catalog/brands');
      await assertCompactTitle(page); { const shot = { path: path.join(output, `brands-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      for (const target of [childCategoryPath, rootCategoryPath, brandPath]) {
        await page.goto(origin + target);
        await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
        await page.getByRole('button', { name: target.includes('/categories/') ? 'Elimina categoria' : 'Elimina marchio', exact: true }).click();
        await page.waitForURL('**?deleted=1');
        await page.getByRole('status').waitFor();
      }
      results.push(`${name}: categories/brands CRUD, hierarchy, filters, duplicate names, WebP logo, invalid upload, deletion and compact titles OK`);
      await page.goto(origin + '/admin/settings/taxes');
      await page.locator('#settings-tab-appearance').click();
      assert.ok(await page.locator('#appearance').isVisible());
      await page.locator('#settings-tab-taxes').click();
      assert.ok(await page.locator('#taxes').isVisible());
      assert.equal(await page.getByLabel('Modalità prezzi', { exact: true }).inputValue(), 'inclusive');
      assert.equal(await page.getByRole('navigation', { name: 'Configurazione imposte', exact: true }).count(), 0);
      assert.equal(await settingsMenu(page).getByRole('link', { name: 'Generali', exact: true }).getAttribute('aria-current'), 'page');
      assert.equal(await settingsMenu(page).getByRole('link', { name: 'Prezzi e imposte', exact: true }).count(), 0);
      await settingsMenu(page).getByRole('link', { name: 'Aliquote', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes/rates');
      assert.equal(await settingsMenu(page).getByRole('link', { name: 'Aliquote', exact: true }).getAttribute('aria-current'), 'page');
      await assertCompactTitle(page); { const shot = { path: path.join(output, `settings-rates-menu-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.getByRole('link', { name: 'Nuova aliquota', exact: false }).click();
      await page.getByLabel('Nome dell’aliquota', { exact: true }).fill(`Aliquota demo ${name}`);
      await page.getByLabel('Aliquota (%)', { exact: true }).fill('50,00');
      await page.getByRole('button', { name: 'Crea aliquota', exact: true }).click();
      await page.waitForURL(/\/admin\/settings\/taxes\/rates\/\d+\?saved=1$/);
      const ratePath = new URL(page.url()).pathname, rateId = ratePath.split('/').at(-1);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `tax-rate-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await settingsMenu(page).getByRole('link', { name: 'Classi fiscali', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes/classes');
      assert.equal(await settingsMenu(page).getByRole('link', { name: 'Classi fiscali', exact: true }).getAttribute('aria-current'), 'page');
      await page.getByRole('link', { name: 'Nuova classe fiscale', exact: false }).click();
      await page.getByLabel('Nome della classe', { exact: true }).fill(`Classe demo ${name}`);
      await page.getByLabel('Aliquota', { exact: true }).selectOption(rateId);
      await page.getByRole('button', { name: 'Crea classe fiscale', exact: true }).click();
      await page.waitForURL(/\/admin\/settings\/taxes\/classes\/\d+\?saved=1$/);
      const classPath = new URL(page.url()).pathname, classId = classPath.split('/').at(-1);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `tax-class-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await openGeneralSettings(page);
      await page.locator('#settings-tab-taxes').click();
      assert.ok(await page.locator('#taxes').isVisible());
      await page.goto(origin + '/admin/settings/taxes');
      await page.getByLabel('Classe fiscale predefinita', { exact: true }).selectOption(classId);
      await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes?saved=1');
      await page.goto(origin + '/admin/settings/taxes/preview');
      assert.equal(await page.getByLabel('Classe fiscale', { exact: true }).inputValue(), classId);
      await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('0,05');
      await page.getByRole('button', { name: 'Calcola', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes/preview');
      assert.equal(await page.locator('[data-total="net"]').innerText(), '0,03 €');
      assert.equal(await page.locator('[data-total="tax"]').innerText(), '0,02 €');
      assert.equal(await page.locator('[data-total="gross"]').innerText(), '0,05 €');
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `tax-inclusive-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const taxErrors = await context.newPage();
      await taxErrors.goto(origin + classPath);
      await taxErrors.getByLabel('Classe attiva', { exact: true }).uncheck();
      const inactiveDefault = taxErrors.waitForResponse(r => r.url().endsWith(classPath) && r.request().method() === 'POST');
      await taxErrors.getByRole('button', { name: 'Salva classe fiscale', exact: true }).click();
      assert.equal((await inactiveDefault).status(), 409);
      assert.ok((await taxErrors.getByRole('alert').innerText()).includes('predefinita'));
      assert.equal(await taxErrors.getByLabel('Classe attiva', { exact: true }).isChecked(), false);
      await taxErrors.goto(origin + ratePath);
      await taxErrors.getByLabel('Aliquota attiva', { exact: true }).uncheck();
      const inactiveRate = taxErrors.waitForResponse(r => r.url().endsWith(ratePath) && r.request().method() === 'POST');
      await taxErrors.getByRole('button', { name: 'Salva aliquota', exact: true }).click();
      assert.equal((await inactiveRate).status(), 409);
      assert.ok((await taxErrors.getByRole('alert').innerText()).includes('classi attive'));
      await taxErrors.goto(origin + '/admin/settings/taxes/rates/new');
      await taxErrors.getByLabel('Nome dell’aliquota', { exact: true }).fill(`Errore ${name}`);
      await taxErrors.getByLabel('Aliquota (%)', { exact: true }).fill('100.01');
      const invalidPercentage = taxErrors.waitForResponse(r => r.url().endsWith('/rates/new') && r.request().method() === 'POST');
      await taxErrors.getByRole('button', { name: 'Crea aliquota', exact: true }).click();
      assert.equal((await invalidPercentage).status(), 422);
      assert.equal(await taxErrors.getByLabel('Aliquota (%)', { exact: true }).getAttribute('aria-invalid'), 'true');
      assert.equal(await taxErrors.getByLabel('Nome dell’aliquota', { exact: true }).inputValue(), `Errore ${name}`);
      await taxErrors.close();
      await page.goto(origin + '/admin/settings/taxes');
      await page.getByLabel('Modalità prezzi', { exact: true }).selectOption('exclusive');
      await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes?saved=1');
      if (name === 'desktop') { await restartPage(page); await page.goto(origin + '/admin/settings/taxes'); assert.equal(await page.getByLabel('Modalità prezzi', { exact: true }).inputValue(), 'exclusive'); }
      await page.goto(origin + '/admin/settings/taxes/preview');
      await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('0.01');
      await page.getByLabel('Quantità', { exact: true }).fill('3');
      await page.getByRole('button', { name: 'Calcola', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes/preview');
      assert.equal(await page.locator('[data-total="net"]').innerText(), '0,03 €');
      assert.equal(await page.locator('[data-total="tax"]').innerText(), '0,02 €');
      assert.equal(await page.locator('[data-total="gross"]').innerText(), '0,05 €');
      await assertCompactTitle(page); { const shot = { path: path.join(output, `tax-exclusive-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.goto(origin + '/admin/settings/taxes');
      await page.getByLabel('Modalità prezzi', { exact: true }).selectOption('inclusive');
      await page.getByLabel('Classe fiscale predefinita', { exact: true }).selectOption('');
      await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes?saved=1');
      await page.goto(origin + '/admin/settings/taxes/rates');
      await page.getByLabel('Cerca per nome', { exact: true }).fill(`Aliquota demo ${name}`);
      await page.getByRole('button', { name: 'Filtra', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes/rates?**');
      assert.equal(await page.locator('.catalog-row').count(), 1);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `tax-rates-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      // Simple products reuse the current class; leave no product blocking later fiscal tests.
      await page.goto(origin + '/admin/catalog/categories/new');
      await page.getByLabel('Nome della categoria', { exact: true }).fill(`Oggetti ${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`oggetti-${name}`);
      await page.getByRole('button', { name: 'Crea categoria', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/categories\/\d+\?saved=1$/);
      const productCategoryPath = new URL(page.url()).pathname, productCategoryId = productCategoryPath.split('/').at(-1);
      await page.goto(origin + '/admin/catalog/brands/new');
      await page.getByLabel('Nome del marchio', { exact: true }).fill(`Atelier ${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`atelier-${name}`);
      await page.getByRole('button', { name: 'Crea marchio', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/brands\/\d+\?saved=1$/);
      const productBrandPath = new URL(page.url()).pathname, productBrandId = productBrandPath.split('/').at(-1);
      await page.goto(origin + '/admin/settings/taxes');
      await page.getByLabel('Classe fiscale predefinita', { exact: true }).selectOption(classId);
      await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes?saved=1');
      await page.goto(origin + '/admin/catalog/products/new');
      assert.equal(await page.getByLabel('Classe fiscale', { exact: true }).inputValue(), classId);
      await page.getByLabel('Nome del prodotto', { exact: true }).fill('Oggetto demo');
      await page.getByLabel('Modello', { exact: true }).fill(`demo-${name}`);
      await page.getByLabel('Slug', { exact: true }).fill(`demo-${name}`);
      await page.getByLabel('Descrizione', { exact: true }).fill('Un oggetto semplice.\nTesto <b>senza HTML</b>.');
      await productSection(page, 'Prezzo e disponibilità');
      await page.getByLabel('Prezzo unitario (€)', { exact: true }).fill('0,05');
      await productSection(page, 'Prezzo e disponibilità');
      await page.getByLabel('Quantità disponibile', { exact: true }).fill('3');
      await productSection(page, 'Organizzazione');
      await page.getByLabel('Categoria (facoltativa)', { exact: true }).selectOption(productCategoryId);
      await productSection(page, 'Organizzazione');
      await page.getByLabel('Marchio', { exact: true }).selectOption(productBrandId);
      await page.getByRole('button', { name: 'Crea prodotto', exact: true }).click();
      await page.waitForURL(/\/admin\/catalog\/products\/\d+\?saved=1&tab=[a-z]+$/);
      const productPath = new URL(page.url()).pathname, publicProductPath = `/products/demo-${name}`;
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404, 'Draft has no public page');
      assert.equal(await page.getByLabel('Modello', { exact: true }).inputValue(), `DEMO-${name.toUpperCase()}`);
      await productSection(page, 'Organizzazione');
      await page.getByLabel('Stato del prodotto', { exact: true }).selectOption('published');
      await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click();
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      await productSection(page, 'Immagine');
      assert.equal(await uploadLibrary(page, 'Seleziona immagine prodotto', 'crates/web/tests/fixtures/webp-lossless.webp'), 303);
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      assert.ok(await page.locator('.product-photo-preview img').evaluate(img => img.complete && img.naturalWidth > 0));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `product-editor-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const productErrors = await context.newPage();
      await productErrors.goto(origin + '/admin/catalog/products/new');
      await productErrors.getByLabel('Nome del prodotto', { exact: true }).fill('Duplicato');
      await productErrors.getByLabel('Modello', { exact: true }).fill(`demo-${name}`);
      await productErrors.getByLabel('Slug', { exact: true }).fill(`duplicate-${name}`);
      const duplicateSku = productErrors.waitForResponse(r => r.url().endsWith('/products/new') && r.request().method() === 'POST');
      await productErrors.getByRole('button', { name: 'Crea prodotto', exact: true }).click();
      assert.equal((await duplicateSku).status(), 409);
      assert.equal(await productErrors.getByLabel('Modello', { exact: true }).getAttribute('aria-invalid'), 'true');
      await productErrors.goto(origin + productPath);
      await productSection(productErrors, 'Immagine');
      assert.equal(await uploadLibrary(productErrors, 'Seleziona immagine prodotto', 'crates/web/tests/fixtures/webp-animated.webp'), 422);
      await productErrors.goto(origin + productPath + '?tab=image');
      assert.equal(await productErrors.locator('.product-photo-preview img').count(), 1);
      await productErrors.goto(origin + '/admin/settings/taxes');
      await productErrors.getByLabel('Modalità prezzi', { exact: true }).selectOption('exclusive');
      const modeBlocked = productErrors.waitForResponse(r => r.url().endsWith('/admin/settings/save') && r.request().method() === 'POST');
      await productErrors.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      assert.equal((await modeBlocked).status(), 409);
      assert.ok((await productErrors.getByRole('alert').innerText()).includes('prodotti'));
      await productErrors.close();
      if (name === 'desktop') { await restartPage(page); await page.goto(origin + productPath); assert.equal(await page.getByLabel('Stato del prodotto', { exact: true }).inputValue(), 'published'); }
      await page.getByRole('link', { name: 'Apri nel negozio', exact: false }).click();
      await page.waitForURL('**' + publicProductPath);
      assert.equal(await page.locator('[data-product-price]').innerText(), '0,05 € IVA inclusa');
      assert.equal(await page.locator('.product-availability').innerText(), 'Disponibile');
      assert.ok(await page.locator('.product-description').innerText().then(text => text.includes('<b>senza HTML</b>')));
      assert.equal(await page.locator('.product-description b').count(), 0);
      assert.ok(await page.locator('.store-detail-photo img').evaluate(img => img.complete && img.naturalWidth > 0));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `product-store-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.locator('main .back-link').click();
      await page.waitForURL('**/catalog');
      await page.getByLabel('Cerca un prodotto', { exact: true }).fill(`demo-${name}`);
      await page.getByRole('button', { name: 'Cerca', exact: true }).click();
      await page.waitForURL('**/search/**');
      assert.equal(await page.locator('.store-product').count(), 1);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page, true); { const shot = { path: path.join(output, `catalog-store-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.goto(origin + productCategoryPath);
      await page.getByLabel('Categoria attiva', { exact: true }).uncheck();
      await page.getByRole('button', { name: 'Salva categoria', exact: true }).click();
      await page.waitForURL('**' + productCategoryPath + '?saved=1');
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404);
      await page.getByLabel('Categoria attiva', { exact: true }).check();
      await page.getByRole('button', { name: 'Salva categoria', exact: true }).click();
      await page.waitForURL('**' + productCategoryPath + '?saved=1');
      await page.goto(origin + productPath);
      await productSection(page, 'Prezzo e disponibilità');
      await page.getByLabel('Quantità disponibile', { exact: true }).fill('0');
      await page.getByRole('button', { name: 'Salva prodotto', exact: true }).click();
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      await page.goto(origin + publicProductPath);
      assert.equal(await page.locator('.product-availability').innerText(), 'Non disponibile');
      await page.goto(origin + '/admin/catalog/products');
      assert.equal(await page.getByRole('link', { name: 'Prodotti', exact: true }).getAttribute('aria-current'), 'page');
      await page.getByLabel('Cerca per nome, modello o slug', { exact: true }).fill(`demo-${name}`);
      await page.getByRole('button', { name: 'Filtra', exact: true }).click();
      await page.waitForURL('**/admin/catalog/products?**');
      assert.equal(await page.locator('.product-table tbody tr').count(), 1);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `product-list-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const createOption = async (label, values) => {
        await page.goto(origin + '/admin/catalog/options/new');
        await page.getByLabel('Nome dell’opzione', { exact: true }).fill(label);
        await page.getByRole('button', { name: 'Crea opzione', exact: true }).click();
        await page.waitForURL(/\/admin\/catalog\/options\/\d+\?saved=1$/);
        const optionPath = new URL(page.url()).pathname, optionId = optionPath.split('/').at(-1), valueIds = [];
        for (const value of values) {
          await page.getByLabel('Nuovo valore', { exact: true }).fill(value);
          await page.getByRole('button', { name: 'Aggiungi valore', exact: true }).click();
          await page.waitForURL('**' + optionPath + '?saved=1');
          const row = page.locator('.option-value-row').filter({ has: page.getByLabel(`Valore ${value}`, { exact: true }) });
          valueIds.push((await row.getByLabel(`Valore ${value}`, { exact: true }).getAttribute('id')).replace('value-', ''));
        }
        assert.equal(await page.getByRole('link', { name: 'Opzioni', exact: true }).getAttribute('aria-current'), 'page');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        await assertCompactTitle(page); { const shot = { path: path.join(output, `options-${name}-${label.startsWith('Colore') ? 'color' : 'size'}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
        return { optionPath, optionId, valueIds };
      };
      const colorOption = await createOption(`Colore ${name}`, ['Rosso', 'Blu', 'Verde non usato']);
      const sizeOption = await createOption(`Taglia ${name}`, ['S', 'M', 'XL non usata']);
      await page.goto(origin + productPath);
      await productSection(page, 'Opzioni e varianti');
      await page.getByLabel('Opzione 1', { exact: true }).selectOption(colorOption.optionId);
      await productSection(page, 'Opzioni e varianti');
      await page.getByLabel('Opzione 2', { exact: true }).selectOption(sizeOption.optionId);
      await page.getByRole('button', { name: 'Salva opzioni prodotto', exact: true }).click();
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404, 'Configured product needs an active variant');
      const makeVariant = async (sku, color, size, price, stock) => {
        await page.goto(origin + productPath + '/variants/new');
        assert.equal(await page.getByLabel('Quantità variante', { exact: true }).inputValue(), '0');
        await page.getByLabel(`Colore ${name}`, { exact: true }).selectOption(color);
        await page.getByLabel(`Taglia ${name}`, { exact: true }).selectOption(size);
        await page.getByLabel('SKU variante', { exact: true }).fill(sku);
        await page.getByLabel('Prezzo variante (€)', { exact: true }).fill(price);
        await page.getByLabel('Quantità variante', { exact: true }).fill(stock);
        await page.getByRole('button', { name: 'Crea variante', exact: true }).click();
        await page.waitForURL(/\/variants\/\d+\?saved=1$/);
        return new URL(page.url()).pathname;
      };
      const redVariantPath = await makeVariant(`v-red-${name}`, colorOption.valueIds[0], sizeOption.valueIds[0], '0,15', '3');
      assert.equal(await uploadLibrary(page, 'Seleziona immagine variante', 'crates/web/tests/fixtures/webp-lossy.webp'), 303);
      await page.waitForURL('**' + redVariantPath + '?saved=1');
      const variantImage = await page.locator('.product-photo-preview img').getAttribute('src');
      assert.ok(await page.locator('.product-photo-preview img').evaluate(img => img.complete && img.naturalWidth > 0));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `variant-editor-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const blueVariantPath = await makeVariant(`v-blue-${name}`, colorOption.valueIds[1], sizeOption.valueIds[1], '0,30', '0');
      const variantErrors = await context.newPage();
      await variantErrors.goto(origin + productPath + '/variants/new');
      await variantErrors.getByLabel(`Colore ${name}`, { exact: true }).selectOption(colorOption.valueIds[0]);
      await variantErrors.getByLabel(`Taglia ${name}`, { exact: true }).selectOption(sizeOption.valueIds[0]);
      await variantErrors.getByLabel('SKU variante', { exact: true }).fill(`dup-${name}`);
      const duplicateCombination = variantErrors.waitForResponse(r => r.url().endsWith('/variants/new') && r.request().method() === 'POST');
      await variantErrors.getByRole('button', { name: 'Crea variante', exact: true }).click();
      assert.equal((await duplicateCombination).status(), 409);
      assert.equal(await variantErrors.getByLabel(`Colore ${name}`, { exact: true }).getAttribute('aria-invalid'), 'true');
      await variantErrors.goto(origin + colorOption.optionPath);
      const redValue = variantErrors.locator('.option-value-row').filter({ has: variantErrors.getByLabel('Valore Rosso', { exact: true }) });
      await redValue.locator('summary').click();
      await redValue.getByLabel('Confermo rimozione Rosso', { exact: true }).check();
      const usedValue = variantErrors.waitForResponse(r => r.url().endsWith('/delete') && r.request().method() === 'POST');
      await redValue.getByRole('button', { name: 'Elimina valore', exact: true }).click();
      assert.equal((await usedValue).status(), 409);
      assert.ok((await variantErrors.getByRole('alert').innerText()).includes('«Rosso»: è utilizzato da una o più varianti'));
      await variantErrors.close();
      if (name === 'desktop') { await page.goto(origin + redVariantPath); await restartPage(page); assert.equal(await page.getByLabel('SKU variante', { exact: true }).inputValue(), `V-RED-${name.toUpperCase()}`); }
      await page.goto(origin + publicProductPath);
      assert.equal(await page.locator('[data-product-price]').innerText(), 'Da 0,15 € IVA inclusa');
      assert.equal(await page.locator('.product-availability').innerText(), 'Seleziona una variante');
      const assertProductValues = async () => {
        assert.equal(await page.getByRole('button', { name: 'Mostra variante', exact: true }).isVisible(), !js, 'Manual variant submission is available only without JavaScript');
        for (const [label, option] of [[`Colore ${name}`, colorOption], [`Taglia ${name}`, sizeOption]]) {
          const values = await page.getByLabel(label, { exact: true }).locator('option').evaluateAll(nodes => nodes.map(n => n.value).filter(v => v !== '0' && v !== ''));
          assert.deepEqual(values.sort(), option.valueIds.slice(0, 2).sort(), 'Only values used by this product, including sold-out variants');
        }
      };
      await assertProductValues();
      const selectVariant = async (color, size) => {
        for (const [label, value] of [[`Colore ${name}`, color], [`Taglia ${name}`, size]]) {
          if (js) {
            await page.waitForFunction(() => typeof window.htmx !== 'undefined');
            await page.evaluate(() => { window.variantSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.variantSettled = true; }, { once: true }); });
            const changed = page.waitForResponse(r => new URL(r.url()).pathname === publicProductPath && r.request().headers()['hx-request'] === 'true');
            await page.getByLabel(label, { exact: true }).focus();
            await page.getByLabel(label, { exact: true }).selectOption(value);
            assert.equal((await changed).status(), 200);
            await page.waitForFunction(({ label, value }) => { const select = [...document.querySelectorAll('select')].find(el => document.querySelector(`label[for="${el.id}"]`)?.textContent === label); return select?.value === value && window.variantSettled && !document.querySelector('.htmx-request'); }, { label, value });
            assert.ok(await page.getByLabel(label, { exact: true }).evaluate(el => el === document.activeElement), 'Selection retains keyboard focus');
          } else { await page.getByLabel(label, { exact: true }).selectOption(value); }
        }
        if (!js) { await page.getByRole('button', { name: 'Mostra variante', exact: true }).click(); await page.waitForURL('**' + publicProductPath + '?**'); }
      };
      await selectVariant(colorOption.valueIds[0], sizeOption.valueIds[0]);
      assert.equal(await page.locator('[data-product-price]').innerText(), '0,15 € IVA inclusa');
      assert.equal(await page.locator('[data-product-sku]').innerText(), `V-RED-${name.toUpperCase()}`);
      assert.equal(await page.locator('.product-availability').innerText(), 'Disponibile');
      assert.ok((await page.locator('.store-detail-photo img').getAttribute('src')).includes(variantImage.split('/').slice(0, -1).join('/')));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `variant-store-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await selectVariant(colorOption.valueIds[0], sizeOption.valueIds[1]);
      assert.ok((await page.locator('#product-detail-message-1').innerText()).includes('Combinazione non disponibile'));
      assert.equal(await page.locator('.store-variant-result').getAttribute('aria-live'),'polite');
      assert.equal(await page.locator('[data-product-model]').innerText(), `DEMO-${name.toUpperCase()}`);
      await selectVariant(colorOption.valueIds[1], sizeOption.valueIds[1]);
      await assertProductValues();
      assert.equal(await page.locator('[data-product-price]').innerText(), '0,30 € IVA inclusa');
      assert.equal(await page.locator('.product-availability').innerText(), 'Non disponibile');
      assert.ok(!(await page.locator('.store-detail-photo img').getAttribute('src')).includes(variantImage.split('/').slice(0, -1).join('/')), 'Variant without photo uses the parent');
      await page.goto(origin + '/catalog');
      await page.getByLabel('Cerca un prodotto', { exact: true }).fill(`v-red-${name}`);
      await page.getByRole('button', { name: 'Cerca', exact: true }).click();
      await page.waitForURL('**/search/**');
      assert.equal(await page.locator('.store-product').count(), 1);
      assert.ok((await page.locator('.store-product-price').innerText()).startsWith('Da 0,15'));
      await page.goto(origin + productPath + '/variants');
      assert.equal(await page.locator('.variant-table tbody tr').count(), 2);
      await assertCompactTitle(page); { const shot = { path: path.join(output, `variants-list-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      for (const target of [redVariantPath, blueVariantPath]) {
        await page.goto(origin + target);
        await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
        await page.getByRole('button', { name: 'Elimina variante', exact: true }).click();
        await page.waitForURL('**/variants?deleted=1');
      }
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404);
      await page.goto(origin + productPath);
      await productSection(page, 'Opzioni e varianti');
      await page.getByLabel('Opzione 1', { exact: true }).selectOption('');
      await productSection(page, 'Opzioni e varianti');
      await page.getByLabel('Opzione 2', { exact: true }).selectOption('');
      await page.getByRole('button', { name: 'Salva opzioni prodotto', exact: true }).click();
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404, 'Removing all options keeps the converted simple product private');
      await productSection(page, 'Organizzazione');
      assert.equal(await page.getByLabel('Stato del prodotto', { exact: true }).inputValue(), 'draft', 'Conversion persists the product as a draft');
      await page.getByLabel('Stato del prodotto', { exact: true }).selectOption('published');
      await Promise.all([
        page.waitForNavigation({ waitUntil: 'load', url: '**' + productPath + '?saved=1&tab=*' }),
        page.getByRole('button', { name: 'Salva prodotto', exact: true }).click(),
      ]);
      assert.equal(await page.getByLabel('Stato del prodotto', { exact: true }).inputValue(), 'published');
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 200, 'Explicit republication restores the simple product public page');
      for (const option of [colorOption, sizeOption]) {
        await page.goto(origin + option.optionPath);
        await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
        await page.getByRole('button', { name: 'Elimina opzione', exact: true }).click();
        await page.waitForURL('**/admin/catalog/options?deleted=1');
      }
      results.push(`${name}: reusable options/values, combinations, duplicate/protected values, variant WebP, price/stock/photo selection, missing combinations, HTMX focus or no-JS submit, SKU search, configured-to-simple draft conversion and explicit republication, real restart and cleanup OK`);
      await page.goto(origin + productPath);
      await productSection(page, 'Immagine');
      await page.getByRole('button', { name: 'Rimuovi immagine prodotto', exact: true }).click();
      await page.waitForURL('**' + productPath + '?saved=1&tab=*');
      assert.equal(await page.locator('.product-photo-preview img').count(), 0);
      await productSection(page, 'Eliminazione');
      await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
      await page.getByRole('button', { name: 'Elimina prodotto', exact: true }).click();
      await page.waitForURL('**/admin/catalog/products?deleted=1');
      assert.equal((await context.request.get(origin + publicProductPath)).status(), 404);
      for (const target of [productCategoryPath, productBrandPath]) {
        await page.goto(origin + target);
        await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
        await page.getByRole('button', { name: target.includes('/categories/') ? 'Elimina categoria' : 'Elimina marchio', exact: true }).click();
        await page.waitForURL('**?deleted=1');
      }
      await page.goto(origin + '/admin/settings/taxes');
      await page.getByLabel('Classe fiscale predefinita', { exact: true }).selectOption('');
      await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await page.waitForURL('**/admin/settings/taxes?saved=1');
      results.push(`${name}: simple products drafts/publication, defaults, duplicate SKU, WebP upload/removal, public prices/search, inactive category, zero stock, price-mode protection, restart and deletion OK`);
      for (const target of [classPath, ratePath]) {
        await page.goto(origin + target);
        await page.getByLabel('Confermo l’eliminazione', { exact: true }).check();
        await page.getByRole('button', { name: target.includes('/classes/') ? 'Elimina classe fiscale' : 'Elimina aliquota', exact: true }).click();
        await page.waitForURL('**?deleted=1');
      }
      results.push(`${name}: rates/classes CRUD, defaults, included/excluded preview, rounding, validation, protected links, persistence and compact titles OK`);
      for (const route of ['/admin/status', '/status']) {
        await page.goto(origin + route);
        if (route === '/admin/status') assert.equal((await page.locator('h1').innerText()).replace(/\s+/g, ' ').trim(), 'Panoramica', 'Dashboard title stays readable on mobile');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${name} layout`);
        await page.locator('#check-status').focus();
        if (js) {
          await page.waitForFunction(() => typeof window.htmx !== 'undefined');
          const response = page.waitForResponse(r => r.url().endsWith(route === '/status' ? '/status' : '/admin/status') && r.request().headers()['hx-request'] === 'true');
          await page.locator('#check-status').press('Enter'); assert.equal((await response).status(), 200);
          await page.locator('[data-state="ok"]').waitFor();
          assert.equal(new URL(page.url()).pathname, route);
          assert.ok(await page.locator('#check-status').evaluate(el => el === document.activeElement));
        } else {
          await page.locator('#check-status').click(); await page.waitForURL('**' + (route === '/status' ? '/status*' : '/admin/status*'));
          await page.locator('[data-state="ok"]').waitFor();
        }
        await assertCompactTitle(page); { const shot = { path: path.join(output, `${route === '/status' ? 'store' : 'admin'}-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      }
      await page.goto(origin + '/admin/roles');
      const navigation = page.getByRole('navigation', { name: 'Amministrazione', exact: true });
      const usersMenu = navigation.locator('.sidebar-group').filter({ has: page.locator('summary', { hasText: /^Utenti$/ }) });
      assert.equal(await navigation.getByRole('link', { name: 'Anteprima negozio', exact: false }).count(), 0);
      assert.ok(await usersMenu.getByRole('link', { name: 'Ruoli', exact: true }).isVisible());
      assert.equal(await usersMenu.getByRole('link', { name: 'Ruoli', exact: true }).getAttribute('aria-current'), 'page');
      await usersMenu.locator('summary').focus();
      await page.keyboard.press('Enter');
      assert.equal(await usersMenu.getByRole('link', { name: 'Lista utenti', exact: true }).isVisible(), false);
      await page.keyboard.press('Enter');
      await usersMenu.getByRole('link', { name: 'Ruoli', exact: true }).click();
      await page.waitForURL('**/admin/roles');
      results.push(`${name}: Users submenu, keyboard navigation, active area and removed store preview OK`);
      await page.getByRole('link', { name: 'Apri ruolo Proprietario', exact: true }).click();
      assert.equal(await page.getByRole('button', { name: 'Salva ruolo', exact: true }).count(), 0);
      assert.equal(await page.getByRole('button', { name: 'Elimina ruolo', exact: true }).count(), 0);
      assert.equal(await page.getByLabel('Nome del ruolo').inputValue(), 'Proprietario');
      assert.ok(await page.getByLabel('Impostazioni: Scrittura', { exact: true }).isChecked());
      assert.ok(await page.getByLabel('Impostazioni: Scrittura', { exact: true }).isDisabled());
      await page.goto(origin + '/admin/roles/new');
      const customRoleName = `Collaboratore ${name}`;
      await page.getByLabel('Nome del ruolo').fill(customRoleName);
      await page.getByLabel('Panoramica: Lettura', { exact: true }).check();
      await page.getByLabel('Impostazioni: Lettura', { exact: true }).check();
      await page.getByRole('button', { name: 'Crea ruolo', exact: true }).click();
      await page.waitForURL('**/admin/roles?saved=1');
      const roleLink = page.getByRole('link', { name: `Apri ruolo ${customRoleName}`, exact: true });
      const rolePath = await roleLink.getAttribute('href');
      const roleKey = rolePath.split('/').at(-1);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `roles-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await roleLink.click();
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `role-edit-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      // Every viewport exercises actual forms; a second session verifies live revocation.
      await page.goto(origin + '/admin');
      await usersMenu.locator('summary').click();
      await usersMenu.getByRole('link', { name: 'Lista utenti', exact: true }).click();
      await page.waitForURL('**/admin/users');
      assert.equal(await page.locator('h1').innerText(), 'Amministratori');
      await page.getByRole('link', { name: 'Nuovo utente', exact: true }).click();
      await page.waitForURL('**/admin/users/new');
      const readerEmail = `reader-${name}@example.test`;
      const readerPassword = randomBytes(20).toString('hex');
      await page.getByLabel('Email amministratore').fill(readerEmail);
      await page.getByLabel('Password iniziale').fill(readerPassword);
      await page.getByLabel('Ruolo', { exact: true }).selectOption(roleKey);
      await page.getByRole('button', { name: 'Crea amministratore', exact: true }).click();
      await page.waitForURL('**/admin/users?saved=1');
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `users-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      const readerContext = await browser.newContext({ viewport, javaScriptEnabled: js });
      const readerPage = await readerContext.newPage();
      const loginReader = async () => {
        await readerPage.goto(origin + '/admin/login');
        await readerPage.getByLabel('Email amministratore').fill(readerEmail);
        await readerPage.getByLabel('Password', { exact: true }).fill(readerPassword);
        await readerPage.getByRole('button', { name: 'Accedi', exact: true }).click();
        await readerPage.waitForURL('**/admin');
      };
      await loginReader();
      assert.equal(await readerPage.getByRole('link', { name: 'Lista utenti', exact: true }).count(), 0);
      assert.equal(await readerPage.locator('.sidebar-group').filter({ has: readerPage.locator('summary', { hasText: /^Utenti$/ }) }).count(), 0);
      await openGeneralSettings(readerPage);
      await readerPage.waitForURL('**/admin/settings');
      assert.equal(await readerPage.getByRole('button', { name: 'Salva impostazioni' }).count(), 0);
      assert.ok(await readerPage.getByText('Il tuo accesso è in sola lettura.', { exact: false }).isVisible());
      assert.ok(await readerPage.getByText('contatti@example.test', { exact: true }).isVisible());
      assert.ok(await readerPage.getByText('Via dei Tigli 12', { exact: true }).isVisible());
      assert.equal(await readerPage.locator('input[name=contact_email]').count(), 0);
      { const shot = { path: path.join(output, `reader-settings-${name}.png`), fullPage: true }; await presentation.verify(readerPage, path.basename(shot.path)); await presentation.screenshot(readerPage, shot); }
      await readerPage.locator('#settings-tab-appearance').click();
      assert.ok(await readerPage.locator('#appearance').isVisible());
      assert.equal(await readerPage.locator('input[type=file]').count(), 0);
      assert.equal(await readerPage.getByRole('button', { name: /Rimuovi|Carica|Sostituisci/ }).count(), 0);
      assert.ok(await readerPage.getByText('Il tuo accesso è in sola lettura.', { exact: false }).isVisible());
      { const shot = { path: path.join(output, `reader-branding-${name}.png`), fullPage: true }; await presentation.verify(readerPage, path.basename(shot.path)); await presentation.screenshot(readerPage, shot); }
      await page.locator('.user-row').filter({ hasText: readerEmail }).getByRole('link', { name: 'Gestisci', exact: false }).click();
      const editUrl = page.url();
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await assertCompactTitle(page); { const shot = { path: path.join(output, `user-edit-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
      await page.getByLabel('Ruolo', { exact: true }).selectOption('owner');
      await page.getByRole('button', { name: 'Salva accesso', exact: true }).click();
      await page.waitForURL('**/admin/users?saved=1');
      await readerPage.goto(origin + '/admin'); await readerPage.waitForURL('**/admin/login');
      await loginReader();
      await readerPage.locator('.sidebar-group').filter({ has: readerPage.locator('summary', { hasText: /^Utenti$/ }) }).locator('summary').click();
      assert.ok(await readerPage.getByRole('link', { name: 'Lista utenti', exact: true }).isVisible());
      await page.goto(editUrl);
      await page.getByLabel('Stato account').selectOption('false');
      await page.getByRole('button', { name: 'Salva accesso', exact: true }).click();
      await page.waitForURL('**/admin/users?saved=1');
      await readerPage.goto(origin + '/admin'); await readerPage.waitForURL('**/admin/login');
      await page.goto(editUrl);
      await page.getByLabel('Stato account').selectOption('true');
      await page.getByLabel('Ruolo', { exact: true }).selectOption(roleKey);
      await page.getByRole('button', { name: 'Salva accesso', exact: true }).click();
      await page.waitForURL('**/admin/users?saved=1');
      await loginReader();
      assert.equal(await readerPage.getByRole('link', { name: 'Lista utenti', exact: true }).count(), 0);
      assert.equal(await readerPage.locator('.sidebar-group').filter({ has: readerPage.locator('summary', { hasText: /^Utenti$/ }) }).count(), 0);
      // Editing a role changes effective rights and revokes every existing session of that role.
      await page.goto(origin + rolePath);
      await page.getByLabel('Nome del ruolo').fill(customRoleName + ' aggiornato');
      await page.getByLabel('Impostazioni: Scrittura', { exact: true }).check();
      await page.getByRole('button', { name: 'Salva ruolo', exact: true }).click();
      await page.waitForURL('**/admin/roles?saved=1');
      await readerPage.goto(origin + '/admin'); await readerPage.waitForURL('**/admin/login');
      await loginReader();
      await readerPage.goto(origin + '/admin/settings');
      assert.ok(await readerPage.getByRole('button', { name: 'Salva impostazioni', exact: true }).isVisible());
      await readerPage.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
      await readerPage.waitForURL('**/admin/settings?saved=1');
      await page.goto(editUrl);
      await page.getByLabel('Ruolo', { exact: true }).selectOption('viewer');
      await page.getByRole('button', { name: 'Salva accesso', exact: true }).click();
      await page.waitForURL('**/admin/users?saved=1');
      await page.goto(origin + rolePath);
      await page.getByRole('button', { name: 'Elimina ruolo', exact: true }).click();
      await page.waitForURL('**/admin/roles?deleted=1');
      assert.equal(await page.getByRole('link', { name: `Apri ruolo ${customRoleName} aggiornato`, exact: true }).count(), 0);
      results.push(`${name}: configurable role CRUD, immutable owner, permission changes and revocation OK`);
      await loginReader();
      await readerPage.goto(origin + '/admin/account');
      const changedEmail = `updated-${name}@example.test`;
      await readerPage.getByLabel('Email', { exact: true }).fill(changedEmail);
      await readerPage.getByLabel('Password attuale per cambiare email', { exact: true }).fill(readerPassword);
      await readerPage.getByRole('button', { name: 'Salva profilo', exact: true }).click();
      await readerPage.waitForURL('**/admin/login');
      await readerPage.getByLabel('Email amministratore').fill(changedEmail);
      await readerPage.getByLabel('Password', { exact: true }).fill(readerPassword);
      await readerPage.getByRole('button', { name: 'Accedi', exact: true }).click();
      await readerPage.waitForURL('**/admin');
      await readerPage.goto(origin + '/admin/account');
      const changedPassword = randomBytes(20).toString('hex');
      await readerPage.getByLabel('Password attuale', { exact: true }).fill(readerPassword);
      await readerPage.getByLabel('Nuova password', { exact: true }).fill(changedPassword);
      await readerPage.getByLabel('Conferma nuova password', { exact: true }).fill(changedPassword);
      await readerPage.getByRole('button', { name: 'Aggiorna password', exact: true }).click();
      await readerPage.waitForURL('**/admin/login');
      await readerPage.getByLabel('Email amministratore').fill(changedEmail);
      await readerPage.getByLabel('Password', { exact: true }).fill(changedPassword);
      await readerPage.getByRole('button', { name: 'Accedi', exact: true }).click();
      await readerPage.waitForURL('**/admin');
      results.push(`${name}: email and password self-service forms, fresh login and reader access OK`);
      await readerContext.close();
      results.push(`${name}: administrator creation, roles, session revocation, deactivation/reactivation and reader permissions OK`);
      await page.goto(origin + '/admin'); await page.getByLabel('Menu account', { exact: true }).click(); await page.getByRole('button', { name: 'Esci', exact: true }).click();
      await page.waitForURL('**/admin/login'); await page.goto(origin + '/admin'); await page.waitForURL('**/admin/login');
      assert.deepEqual(errors, []); await context.close(); results.push(`${name}: login, forms, layout, status, logout and console OK`);
    }
    const validation = await browser.newContext(); validation.on('page',p=>p.on('dialog',d=>d.accept())); const validationPage = await validation.newPage();
    await validationPage.goto(origin + '/admin/login');
    await validationPage.getByLabel('Email amministratore').fill(email);
    await validationPage.getByLabel('Password', { exact: true }).fill(password);
    await validationPage.getByRole('button', { name: 'Accedi', exact: true }).click(); await validationPage.waitForURL('**/admin');
    await openGeneralSettings(validationPage);
    await validationPage.getByLabel('Nome del negozio').fill('   ');
    await validationPage.getByLabel('Email di contatto', { exact: true }).fill('bad-email');
    await validationPage.getByRole('textbox', { name: 'Indirizzo', exact: true }).fill('Via conservata 99');
    await validationPage.locator('#settings-editor').evaluate(form => { form.noValidate = true; });
    const invalidSettings = validationPage.waitForResponse(r => r.url().endsWith('/admin/settings/save') && r.request().method() === 'POST');
    await validationPage.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
    assert.equal((await invalidSettings).status(), 422);
    await validationPage.getByRole('alert').waitFor();
    assert.equal(await validationPage.getByLabel('Nome del negozio').inputValue(), '   ');
    assert.equal(await validationPage.getByLabel('Email di contatto', { exact: true }).inputValue(), 'bad-email');
    assert.equal(await validationPage.getByRole('textbox', { name: 'Indirizzo', exact: true }).inputValue(), 'Via conservata 99');
    assert.equal(await validationPage.getByLabel('Email di contatto', { exact: true }).getAttribute('aria-invalid'), 'true');
    assert.equal(await validationPage.getByLabel('Email di contatto', { exact: true }).getAttribute('aria-describedby'), 'error-contact_email');
    assert.ok(await validationPage.locator('#error-contact_email').isVisible());
    { const shot = { path: path.join(output, 'settings-fields-error.png'), fullPage: true }; await presentation.verify(validationPage, path.basename(shot.path)); await presentation.screenshot(validationPage, shot); }
    await validationPage.goto(origin + '/admin/settings');
    assert.equal(await validationPage.getByLabel('Nome del negozio').inputValue(), 'Atelier persistente');
    assert.equal(await validationPage.getByRole('textbox', { name: 'Indirizzo', exact: true }).inputValue(), 'Via dei Tigli 12');
    await validationPage.getByLabel('Nome del negozio').fill('   ');
    await validationPage.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
    await validationPage.getByRole('alert').waitFor();
    { const shot = { path: path.join(output, 'settings-error.png'), fullPage: true }; await presentation.verify(validationPage, path.basename(shot.path)); await presentation.screenshot(validationPage, shot); }
    await validationPage.goto(origin + '/admin/users');
    await validationPage.locator('.user-row').filter({ hasText: email }).getByRole('link', { name: 'Gestisci', exact: false }).click();
    await validationPage.getByLabel('Ruolo', { exact: true }).selectOption('viewer');
    const lastOwner = validationPage.waitForResponse(r => r.request().method() === 'POST');
    await validationPage.getByRole('button', { name: 'Salva accesso', exact: true }).click();
    assert.equal((await lastOwner).status(), 409);
    await validationPage.getByRole('alert').waitFor();
    assert.ok(await validationPage.getByRole('alert').innerText().then(text => text.includes('almeno un proprietario attivo')));
    await validationPage.goto(origin + '/admin/roles/viewer');
    const assignedRole = validationPage.waitForResponse(r => r.request().method() === 'POST');
    await validationPage.getByRole('button', { name: 'Elimina ruolo', exact: true }).click();
    assert.equal((await assignedRole).status(), 409);
    await validationPage.getByRole('alert').waitFor();
    assert.ok(await validationPage.getByRole('alert').innerText().then(text => text.includes('Riassegna tutti gli account')));
    await validation.close(); results.push('Validation feedback and last-owner protection through real forms: OK');
    const recovery = await browser.newContext(); const resetPage = await recovery.newPage();
    const resetFile = path.join(temp, 'reset'); cli('reset-token', '--email', email, '--output', resetFile);
    assert.equal(fs.statSync(resetFile).mode & 0o777,0o600);
    const refusal=spawnSync(binary,['--config',config,'reset-token','--email',email,'--output',resetFile],{env,encoding:'utf8'});assert.notEqual(refusal.status,0,'Recovery token files cannot be overwritten');
    await resetPage.goto(origin + '/admin/reset');
    await resetPage.getByLabel('Codice di recupero').fill(fs.readFileSync(resetFile, 'utf8'));
    const newPassword = randomBytes(20).toString('hex');
    await resetPage.getByLabel('Password', { exact: true }).fill(newPassword);
    await resetPage.getByRole('button', { name: 'Salva nuova password' }).click(); await resetPage.waitForURL('**/admin/login');
    await resetPage.getByLabel('Email amministratore').fill(email);
    await resetPage.getByLabel('Password', { exact: true }).fill(newPassword);
    await resetPage.getByRole('button', { name: 'Accedi' }).click(); await resetPage.waitForURL('**/admin');
    await recovery.close(); results.push('Operator-assisted recovery and login with new password: OK');
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2));
    console.log(results.join('\n'));
  } finally {
    if (browser) await browser.close();
    await stop();
    await owner.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    await owner.end(); fs.rmSync(temp, { recursive: true, force: true });
  }
})().catch(error => { console.error(error.message); console.error((error.stack || '').split('\n').filter(line => line.includes('browser-admin-foundation.cjs:')).join('\n')); process.exitCode = 1; });
