// Isolated synthetic catalog: option/value priorities, EN/IT, themes and native/HTMX selection.
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
  const schema = `talocart_option_order_${process.pid}_${Date.now()}`;
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'talocart-option-order-'));
  const origin = 'http://127.0.0.1:3146';
  const output = path.resolve('.local/verification/option-order');
  fs.mkdirSync(output, { recursive: true });
  const url = new URL(process.env.TALOCART_TEST_DATABASE_URL);
  url.searchParams.set('options', `-csearch_path=${schema}`);
  const config = path.join(folder, 'talocart.toml');
  fs.writeFileSync(config, '[server]\nlisten="127.0.0.1:3146"\n', { mode: 0o600 });
  const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/debug/talocart');
  const env = { ...process.env, TALOCART_DATABASE_URL: url.href, TALOCART_LISTEN: '127.0.0.1:3146', TALOCART_MEDIA_ROOT: path.join(folder, 'media'), TALOCART_LOG: 'warn' };
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
    await admin.getByLabel('Nome del negozio', { exact: true }).fill('Option ordering');
    await admin.getByLabel('Email amministratore').fill('owner@example.test');
    await admin.getByLabel('Password', { exact: true }).fill(password);
    await admin.getByLabel('Installa dati demo', { exact: true }).check();
    await admin.getByRole('button', { name: 'Crea negozio', exact: true }).click();
    await admin.waitForURL('**/admin/login');
    await admin.getByLabel('Email amministratore').fill('owner@example.test');
    await admin.getByLabel('Password', { exact: true }).fill(password);
    await admin.getByRole('button', { name: 'Accedi', exact: true }).click();
    await admin.waitForURL('**/admin');
    const cookies = (await setup.cookies()).filter(cookie => cookie.name === 'talocart_session');
    const color = (await db.query("INSERT INTO catalog_options(name) VALUES('AA Color') RETURNING id")).rows[0].id;
    const size = (await db.query("INSERT INTO catalog_options(name) VALUES('ZZ Size') RETURNING id")).rows[0].id;
    const values = {};
    for (const [option, names] of [[size, ['XL', 'S', 'M']], [color, ['Red', 'Blue']]]) {
      for (const name of names) values[name] = (await db.query('INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id', [option, name])).rows[0].id;
    }
    const product = (await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,category_id,brand_id,tax_class_id) SELECT 'Ordered product','ORDERED','ordered-product',100,10,'published',category_id,brand_id,tax_class_id FROM catalog_products WHERE model='MUG' RETURNING id")).rows[0].id;
    await db.query('INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2),($1,2,$3)', [product, color, size]);
    for (const sizeName of ['S', 'M', 'XL']) for (const colorName of ['Blue', 'Red']) {
      await db.query('INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint,$3::bigint],$4,100,10)', [product, values[sizeName], values[colorName], `${sizeName}-${colorName.toUpperCase()}`]);
    }
    const material = (await db.query("INSERT INTO catalog_options(name) VALUES('Material') RETURNING id")).rows[0].id;
    const cotton = (await db.query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Cotton') RETURNING id", [material])).rows[0].id;
    const threeAxes = (await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,category_id,brand_id,tax_class_id) SELECT 'Three options','THREE','three-options',100,10,'published',category_id,brand_id,tax_class_id FROM catalog_products WHERE model='MUG' RETURNING id")).rows[0].id;
    await db.query('INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2),($1,2,$3),($1,3,$4)', [threeAxes, color, size, material]);
    await db.query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint,$3::bigint,$4::bigint],'THREE-COTTON',100,10)", [threeAxes, values.Blue, values.M, cotton]);
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) for (const [profile, width, js] of [['desktop', 1440, true], ['mobile', 390, true], ['no-js', 1440, false], ['mobile-no-js', 390, false]]) {
      await db.query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await db.query('UPDATE administrators SET locale=$1', [locale]);
      await db.query('UPDATE catalog_options SET sort_order=0 WHERE id=ANY($1::bigint[])', [[size, color]]);
      await db.query('UPDATE catalog_option_values SET sort_order=0 WHERE option_id=ANY($1::bigint[])', [[size, color]]);
      const context = await browser.newContext({ viewport: { width, height: 1000 }, javaScriptEnabled: js });
      await context.addCookies(cookies);
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      try {
        const orderLabel = locale === 'it' ? 'Ordinamento' : 'Sort order';
        for (const [option, order] of [[color, '20'], [size, '10']]) {
          await page.goto(origin + `/admin/catalog/options/${option}`);
          assert.equal(await page.locator('html').getAttribute('lang'), locale);
          const control = page.locator('#option-sort-order');
          assert.equal(await control.getAttribute('name'), 'sort_order');
          assert.equal(await page.locator('label[for=option-sort-order]').innerText(), orderLabel);
          await control.focus();
          assert.ok(await control.evaluate(el => el === document.activeElement));
          if (option === color) {
            await control.fill('10000');
            await page.locator('.option-editor form').evaluate(form => { form.noValidate = true; });
            const [response] = await Promise.all([page.waitForNavigation(), page.locator('.option-editor button[type=submit]').click()]);
            assert.equal(response.status(), 422);
            assert.equal(await control.inputValue(), '10000');
            assert.equal(await control.getAttribute('aria-invalid'), 'true');
            assert.ok((await page.getByRole('alert').innerText()).includes(locale === 'it' ? 'Inserisci un ordinamento intero' : 'Enter a whole-number sort order'));
          }
          await control.fill(order);
          await Promise.all([page.waitForNavigation(), page.locator('.option-editor button[type=submit]').click()]);
          assert.equal(await control.inputValue(), order);
        }
        for (const [name, order] of [['S', '10'], ['M', '20'], ['XL', '30']]) {
          const control = page.locator(`#value-${values[name]}-sort-order`);
          await control.fill(order);
          await Promise.all([page.waitForNavigation(), page.locator(`form[action$="/values/${values[name]}"] button[type=submit]`).click()]);
        }
        assert.deepEqual(await page.locator('.option-value-edit input[name=name]').evaluateAll(items => items.map(item => item.value)), ['S', 'M', 'XL']);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await page.screenshot({ path: path.join(output, `${theme}-${locale}-${profile}-admin.png`), fullPage: true });
        const storePath = (locale === 'en' ? '/en' : '') + '/products/ordered-product';
        await page.goto(origin + storePath);
        const checkOrder = async () => {
          assert.deepEqual(await page.locator('.store-variant-selector > label').allTextContents(), ['ZZ Size', 'AA Color']);
          assert.deepEqual(await page.getByLabel('ZZ Size', { exact: true }).locator('option').evaluateAll(items => items.slice(1).map(item => item.textContent)), ['S', 'M', 'XL']);
        };
        await checkOrder();
        assert.equal(await page.getByLabel('ZZ Size', { exact: true }).getAttribute('name'), 'value_2');
        assert.equal(await page.getByLabel('AA Color', { exact: true }).getAttribute('name'), 'value_1');
        // A saved URL retains the same selection after the visual axis order changes.
        await page.goto(origin + storePath + `?value_1=${values.Red}&value_2=${values.XL}`);
        assert.equal(await page.locator('[data-product-sku]').innerText(), 'XL-RED');
        await checkOrder();
        for (const [name, value] of [['ZZ Size', values.M], ['AA Color', values.Blue]]) {
          const select = page.getByLabel(name, { exact: true });
          await select.focus();
          if (js) {
            await page.evaluate(() => { window.optionOrderSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.optionOrderSettled = true; }, { once: true }); });
            const response = page.waitForResponse(response => response.request().headers()['hx-request'] === 'true' && new URL(response.url()).pathname === storePath);
            await select.selectOption(String(value));
            assert.equal((await response).status(), 200);
            await page.waitForFunction(() => window.optionOrderSettled === true);
            assert.ok(await select.evaluate(el => el === document.activeElement), 'Focus survives HTMX');
          } else await select.selectOption(String(value));
        }
        if (!js) {
          await page.getByRole('button', { name: locale === 'it' ? 'Mostra variante' : 'Show variant', exact: true }).click();
          await page.waitForURL(url => url.searchParams.get('value_1') === String(values.Blue));
        }
        await checkOrder();
        assert.equal(await page.locator('[data-product-sku]').innerText(), 'M-BLUE');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        assert.deepEqual(errors, []);
        await page.screenshot({ path: path.join(output, `${theme}-${locale}-${profile}-store.png`), fullPage: true });
        // Three axes used to split the last label from its control in the native fallback.
        const threePath = (locale === 'en' ? '/en' : '') + '/products/three-options';
        await page.goto(origin + threePath);
        const geometry = await page.locator('.store-variant-selector').evaluate(form => {
          const rect = element => { const r = element.getBoundingClientRect(); return { x: r.x, y: r.y, bottom: r.bottom }; };
          return {
            pairs: Array.from(form.querySelectorAll('label'), label => ({ label: rect(label), select: rect(document.getElementById(label.htmlFor)) })),
            button: form.querySelector('button') ? rect(form.querySelector('button')) : null,
          };
        });
        assert.equal(geometry.pairs.length, 3);
        for (const pair of geometry.pairs) {
          assert.ok(Math.abs(pair.label.x - pair.select.x) < 1, 'Each option label aligns with its selector');
          assert.ok(pair.select.y >= pair.label.bottom, 'Each selector follows its own label');
        }
        if (!js) assert.ok(geometry.button.y >= Math.max(...geometry.pairs.map(pair => pair.select.bottom)), 'Native submit follows every selector');
        for (const [field, value] of [['value_1', values.Blue], ['value_2', values.M], ['value_3', cotton]]) {
          const select = page.locator(`.store-variant-selector [name="${field}"]`);
          await select.focus();
          if (js) {
            await page.evaluate(() => { window.threeOptionsSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.threeOptionsSettled = true; }, { once: true }); });
            const response = page.waitForResponse(r => r.request().headers()['hx-request'] === 'true' && new URL(r.url()).pathname === threePath);
            await select.selectOption(String(value));
            assert.equal((await response).status(), 200);
            await page.waitForURL(url => url.searchParams.get(field) === String(value));
            await page.waitForFunction(() => window.threeOptionsSettled === true);
            assert.ok(await select.evaluate(el => el === document.activeElement), 'Three-option focus survives HTMX');
          } else await select.selectOption(String(value));
        }
        if (!js) {
          await page.getByRole('button', { name: locale === 'it' ? 'Mostra variante' : 'Show variant', exact: true }).click();
          await page.waitForURL(url => url.searchParams.get('value_3') === String(cotton));
        }
        assert.equal(await page.locator('[data-product-sku]').innerText(), 'THREE-COTTON');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await page.screenshot({ path: path.join(output, `${theme}-${locale}-${profile}-three-options.png`), fullPage: true });
        results.push({ theme, locale, profile, passed: true });
        console.log(theme, locale, profile, 'passed');
      } finally { await context.close(); }
    }
    for (const locale of ['it', 'en']) {
      await db.query('UPDATE administrators SET locale=$1', [locale]);
      await admin.goto(origin + '/admin/catalog/options/new');
      await admin.locator('#option-name').fill(`New ordered ${locale}`);
      await admin.locator('#option-sort-order').fill('7');
      await Promise.all([admin.waitForNavigation(), admin.locator('.option-editor button[type=submit]').click()]);
      const id = new URL(admin.url()).pathname.split('/').pop();
      assert.equal((await db.query('SELECT sort_order FROM catalog_options WHERE id=$1', [id])).rows[0].sort_order, 7);
      await admin.locator('#new-value').fill('New ordered value');
      await admin.locator('#new-value-sort-order').fill('9');
      await Promise.all([admin.waitForNavigation(), admin.locator('.option-new-value button[type=submit]').click()]);
      assert.equal((await db.query('SELECT sort_order FROM catalog_option_values WHERE option_id=$1', [id])).rows[0].sort_order, 9);
    }
    await setup.close();
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify(results, null, 2));
  } finally {
    if (server && server.exitCode === null) { const done = once(server, 'exit'); server.kill('SIGTERM'); await done; }
    await browser.close();
    await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
    await db.end();
    fs.rmSync(folder, { recursive: true, force: true });
  }
})().catch(error => { console.error(error.stack); process.exitCode = 1; });
