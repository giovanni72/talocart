const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, output, name, js }) => {
  const [source, target] = (await query('SELECT id,slug,name FROM catalog_visible_products ORDER BY id LIMIT 2')).rows;
  const editor = `${origin}/admin/catalog/products/${source.id}`;
  const route = `${editor}?tab=related#panel-related`;
  const panel = page.locator('#panel-related');
  const waitForTab = () => page.waitForURL(url => url.pathname === `/admin/catalog/products/${source.id}` && url.hash === '#panel-related');
  const submit = async action => {
    const [response] = await Promise.all([page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/related')), page.waitForNavigation({waitUntil: 'load'}), action()]);
    assert.equal(response.status(), 303);
  };
  const saveProduct = async () => {
    const [response] = await Promise.all([page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === `/admin/catalog/products/${source.id}`), page.waitForNavigation({waitUntil: 'load'}), page.locator('[data-product-save] button').click()]);
    assert.equal(response.status(), 303);
    await page.locator('#tab-related').click();
    await waitForTab();
  };
  const original = (await query('SELECT theme_id FROM storefront_settings WHERE id=1')).rows[0].theme_id;
  try {
    for (const theme of ['default', 'essential']) {
      await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [theme]);
      for (const lang of ['it', 'en']) {
        await query('UPDATE administrators SET locale=$1', [lang]);
        await page.goto(`${origin}/admin/components`);
        assert.equal((await page.locator('main').count()), 1);
        await query('DELETE FROM catalog_related_products WHERE product_id=$1', [source.id]);
        await page.goto(`${origin}/admin/catalog/products/${source.id}`);
        const tabIds = await page.locator('.ui-tabs__tab').evaluateAll(tabs => tabs.map(tab => tab.id));
        assert.equal(tabIds[tabIds.indexOf('tab-variants') + 1], 'tab-related');
        await page.locator('#tab-related').click();
        await waitForTab();
        if (js) {
          assert.equal(await page.locator('#tab-related').getAttribute('aria-selected'), 'true');
          assert.equal(await page.locator('[data-product-save]').isVisible(), true);
          await page.locator('#tab-variants').click();
          await page.locator('#tab-variants').press('ArrowRight');
          assert.equal(await page.locator('#tab-related').getAttribute('aria-selected'), 'true');
        }
        assert.ok(await panel.locator('button').evaluateAll(buttons => buttons.every(button => button.classList.contains('ui-button--mini') && button.getBoundingClientRect().height < 40)));
        await waitForTab();
        assert.equal(await panel.locator('.related-search .related-row').count(), 0);
        assert.equal(await panel.locator('[data-related-search-idle]').count(), 1);
        await page.locator('#related-fallback').selectOption('either');
        assert.equal(await page.locator('#related-fallback').evaluate(e => e.form.id), 'product-editor-form');
        assert.equal(await panel.locator('button[value="fallback"]').count(), 0);
        await saveProduct();
        await waitForTab();
        await page.waitForLoadState('load');
        assert.equal(await page.locator('#related-fallback').inputValue(), 'either');
        await page.goto(`${origin}${lang === 'it' ? '' : '/en'}/products/${source.slug}`);
        const automaticCount = await page.locator('section[aria-labelledby="related-title"] .store-product').count();
        assert.equal(automaticCount, 4);
        await page.goto(route);
        await page.locator('#related-search').fill(target.name);
        await Promise.all([page.waitForNavigation({waitUntil:'load'}), panel.locator('form[method="get"] button').click()]);
        await page.waitForURL(url => url.searchParams.get('related_q') === target.name);
        const add = panel.locator('form').filter({ has: page.locator(`input[name="target"][value="${target.id}"]`) }).locator('button[value="add"]');
        await submit(() => add.click());
        await waitForTab();
        assert.equal(await page.locator('#related-search').inputValue(), target.name);
        const remove = panel.locator('button[value="remove"]');
        await remove.waitFor();
        assert.equal(await remove.count(), 1);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await panel.screenshot({ path: path.join(output, `related-admin-${theme}-${lang}-${name}.png`), });
        await page.goto(`${origin}${lang === 'it' ? '' : '/en'}/products/${source.slug}`);
        const section = page.locator('section[aria-labelledby="related-title"]');
        assert.equal(await section.locator('.store-product').count(), 1);
        assert.ok(await section.locator(`a[href$="/products/${target.slug}"]`).count() > 0);
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        await section.scrollIntoViewIfNeeded();
        for (const image of await section.locator('img').all()) await image.evaluate(img => img.decode());
        await section.screenshot({ path: path.join(output, `related-store-${theme}-${lang}-${name}.png`) });
        await page.goto(route);
        await submit(() => panel.locator('button[value="remove"]').click());
        await waitForTab();
        await page.waitForLoadState('load');
        assert.equal(await panel.locator('button[value="remove"]').count(), 0);
        await page.locator('#related-fallback').selectOption('none');
        await page.locator('#tab-general').click();
        await saveProduct();
        await waitForTab();
        await page.goto(`${origin}${lang === 'it' ? '' : '/en'}/products/${source.slug}`);
        assert.equal(await page.locator('#related-title').count(), 0);
      }
    }
  } finally {
    await query('DELETE FROM catalog_related_products WHERE product_id=$1', [source.id]);
    await query('DELETE FROM catalog_related_settings WHERE product_id=$1', [source.id]);
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [original]);
    await query("UPDATE administrators SET locale='it'");
  }
};
