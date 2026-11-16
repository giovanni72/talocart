const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, context, query, origin, output, name, js, viewport }) => {
  const settings = '/admin/settings/storefront';
  const admin = await context.newPage();
  const form = () => admin.locator('#settings-editor');
  const save = async (primary, active) => {
    await admin.goto(origin + settings);
    await form().getByRole('combobox', { name: 'Lingua principale', exact: true }).selectOption(primary);
    for (const [code, label] of [['it', 'Italiano'], ['en', 'English']]) await form().getByRole('checkbox', { name: label, exact: true }).setChecked(active.includes(code));
    await form().getByRole('button', { name: 'Salva impostazioni', exact: true }).focus();
    await admin.keyboard.press('Enter');
    await admin.waitForURL(url => url.pathname === settings && url.searchParams.get('saved') === '1');
  };
  const selectVariantAndPhoto = async () => {
    for (const [label, value] of [['Colore', 'Blu'], ['Taglia', 'M']]) {
      if (js) await page.evaluate(() => { window.policySettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.policySettled = true; }, { once: true }); });
      await page.getByLabel(label, { exact: true }).selectOption({ label: value });
      if (js) await page.waitForFunction(() => window.policySettled);
    }
    if (!js) { await page.locator('.store-variant-selector button').click(); await page.waitForURL(url => url.searchParams.has('value_2')); }
    assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
    await page.locator('.store-photo-thumbnails a').last().click();
    await page.waitForURL(url => Boolean(url.searchParams.get('photo')));
    return new URL(page.url());
  };
  const original = (await query("SELECT (to_jsonb(s)-'primary_locale')::text AS data FROM storefront_settings s")).rows[0].data;
  try {
    await admin.goto(origin + settings);
    assert.equal(await form().getByRole('combobox', { name: 'Lingua principale', exact: true }).inputValue(), 'it');
    await form().getByRole('checkbox', { name: 'Italiano', exact: true }).uncheck();
    let rejected = admin.waitForResponse(response => response.url().endsWith('/admin/settings/save') && response.request().method() === 'POST');
    await form().getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
    assert.equal((await rejected).status(), 422);
    assert.equal(await form().getByRole('checkbox', { name: 'Italiano', exact: true }).isChecked(), false);
    assert.equal(await form().getByRole('combobox', { name: 'Lingua principale', exact: true }).getAttribute('aria-describedby'), 'primary-locale-error');
    await form().getByRole('checkbox', { name: 'English', exact: true }).uncheck();
    rejected = admin.waitForResponse(response => response.url().endsWith('/admin/settings/save') && response.request().method() === 'POST');
    await form().getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
    assert.equal((await rejected).status(), 422);
    assert.ok(await admin.locator('#active-locales-error').isVisible());
    await page.goto(origin + '/products/tshirt-atelier');
    await save('en', ['en']);
    if (js) {
      const partial = page.waitForResponse(response => new URL(response.url()).pathname === '/products/tshirt-atelier' && response.request().headers()['hx-request'] === 'true');
      await page.getByLabel('Colore', { exact: true }).selectOption({ label: 'Blu' });
      const response = await partial;
      assert.equal(response.status(), 200);
      assert.ok(response.headers()['hx-redirect'], 'A stale public language snapshot triggers a full document refresh');
      await page.waitForFunction(() => document.documentElement.lang === 'en');
      assert.equal(await page.locator('.store-languages').count(), 0);
    }

    for (const route of ['/', '/catalog', '/brands', '/products/tazza-atelier']) {
      const response = await page.goto(origin + route); assert.equal(response.status(), 200);
      assert.equal(await page.locator('html').getAttribute('lang'), 'en');
      assert.equal(await page.locator('.store-languages').count(), 0);
      assert.equal(await page.locator('link[hreflang="it"]').count(), 0);
      assert.equal(await page.locator('link[rel="canonical"]').getAttribute('href'), origin + route);
      assert.equal(await page.locator('#store-search-form').getAttribute('action'), '/search');
    }
    assert.equal(await page.locator('[data-product-price]').innerText(), '14.90 € VAT included');
    const alias = '/en/catalog?available=true&page=2&per_page=21';
    const redirect = await context.request.get(origin + alias, { maxRedirects: 0 });
    assert.equal(redirect.status(), 301); assert.equal(redirect.headers().location, alias.slice(3));
    await page.goto(origin + alias); assert.equal(new URL(page.url()).pathname, '/catalog');
    const map = await (await context.request.get(origin + '/sitemap-products.xml?page=1')).text();
    assert.equal((map.match(/<loc>/g) || []).length, 27); assert.ok(!map.includes('/en/products/'));
    await page.goto(origin + '/products/tshirt-atelier');
    const single = await selectVariantAndPhoto(); assert.equal(single.pathname, '/products/tshirt-atelier');
    assert.equal(await page.locator('[data-product-price]').innerText(), '26.90 € VAT included');
    await save('en', ['it', 'en']);
    assert.ok((await admin.locator('.store-language-options label').evaluateAll(labels => labels.map(label => label.getBoundingClientRect().height))).every(height => height >= 44));
    if (viewport.width <= 500) await form().scrollIntoViewIfNeeded();
    await admin.screenshot({ path: path.join(output, `store-language-settings-${name}.png`), fullPage: false });
    await page.goto(origin + '/it/catalog?available=true&page=2&per_page=21');
    assert.equal(await page.locator('html').getAttribute('lang'), 'it');
    const filtered = new URL(page.url());
    await page.getByRole('navigation', { name: 'Lingua del negozio' }).getByRole('link', { name: 'English', exact: true }).focus();
    await page.keyboard.press('Enter'); await page.waitForURL(url => url.pathname === '/catalog');
    assert.equal(new URL(page.url()).search, filtered.search);
    assert.equal(await page.locator('html').getAttribute('lang'), 'en');
    await page.getByRole('searchbox').fill('Tazza'); await page.getByRole('button', { name: 'Search', exact: true }).click();
    await page.waitForURL(url => url.pathname === '/search/Tazza'); assert.equal(new URL(page.url()).searchParams.get('available'), 'true');
    await page.goto(origin + '/it/products/tshirt-atelier');
    const chosen = await selectVariantAndPhoto(); assert.equal(chosen.pathname, '/it/products/tshirt-atelier');
    assert.equal(await page.locator('.store-languages a[hreflang="en"]').getAttribute('href'), chosen.pathname.slice(3) + chosen.search);
    await page.getByRole('navigation', { name: 'Lingua del negozio' }).getByRole('link', { name: 'English', exact: true }).click();
    await page.waitForURL(url => url.pathname === '/products/tshirt-atelier');
    assert.equal(new URL(page.url()).search, chosen.search); assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
    await page.goBack(); assert.equal(new URL(page.url()).pathname, chosen.pathname); assert.equal(await page.locator('html').getAttribute('lang'), 'it');
    await page.goForward(); assert.equal(new URL(page.url()).pathname, '/products/tshirt-atelier'); assert.equal(await page.locator('html').getAttribute('lang'), 'en');
    for (const width of [320, 390, 800, 1440, 1920]) {
      await page.setViewportSize({ width, height: 1050 });
      if (js) await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Primary English detail at ${width}px`);
    }
    await page.setViewportSize(viewport); await page.goto(origin + '/catalog');
    await page.screenshot({ path: path.join(output, `store-primary-english-${name}.png`), fullPage: false });
    await save('it', ['it']);
    const disabled = await context.request.get(origin + '/en/catalog?per_page=42', { maxRedirects: 0 });
    assert.equal(disabled.status(), 301); assert.equal(disabled.headers().location, '/catalog?per_page=42');
    await page.goto(origin + '/catalog'); assert.equal(await page.locator('html').getAttribute('lang'), 'it'); assert.equal(await page.locator('.store-languages').count(), 0);
  } finally {
    await save('it', ['it', 'en']); await admin.close();
  }
  await page.goto(origin + '/en/catalog'); assert.equal(new URL(page.url()).pathname, '/en/catalog'); assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  assert.equal((await query("SELECT (to_jsonb(s)-'primary_locale')::text AS data FROM storefront_settings s")).rows[0].data, original);
  assert.equal((await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale, 'it');
  await page.goto(origin + '/catalog');
};
