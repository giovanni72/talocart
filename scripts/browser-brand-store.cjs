const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, js, output, name }) => {
  await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
  const viewport = page.viewportSize();
  const navigation = page.locator('.category-navigation');
  const dropdown = page.locator('.brand-dropdown');
  const summary = dropdown.locator(':scope > summary');
  const openMenu = async () => {
    if (!await navigation.evaluate(el => el.open)) {
      await navigation.locator(':scope > summary').focus(); await page.keyboard.press('Enter');
    }
    if (!await dropdown.evaluate(el => el.open)) {
      await summary.focus(); await page.keyboard.press('Enter');
    }
  };
  const brand = (await query("SELECT id,name,description,logo_key FROM catalog_brands WHERE slug='carta-e-forme'")).rows[0];
  const inserted = (await query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) SELECT 'ZZ Marchio test '||n,'BRAND-VIEW-'||n,'brand-view-'||n,'published',CASE WHEN n=25 THEN NULL ELSE p.category_id END,p.brand_id,p.tax_class_id,100,5 FROM (SELECT category_id,brand_id,tax_class_id FROM catalog_visible_products p JOIN catalog_brands b ON b.id=p.brand_id WHERE b.slug='carta-e-forme' LIMIT 1) p CROSS JOIN generate_series(1,25) n RETURNING id")).rows.map(r => r.id);
  const route = origin + '/brands/carta-e-forme';
  try {
    await query("WITH u AS (UPDATE catalog_brands SET description=$1,logo_key=coalesce(logo_key,(SELECT image_key FROM catalog_products WHERE image_key IS NOT NULL AND brand_id=$2 LIMIT 1)) WHERE id=$2 RETURNING id,description) UPDATE catalog_brands_translations t SET description=u.description FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)", ['Carta, quaderni e idee per i tuoi progetti.', brand.id]);
    for (const url of ['/', '/catalog', '/catalog/cancelleria', '/products/tazza-atelier', '/brands']) {
      await page.goto(origin + url);
      assert.equal(await page.locator('.category-menu > li:last-child.brand-menu-item').count(), 1);
      await openMenu();
      assert.ok(await summary.isVisible());
      const expected = (await query('SELECT name FROM catalog_brands WHERE active ORDER BY lower(name),id LIMIT 100')).rows.map(r => r.name);
      assert.deepEqual(await dropdown.locator('.brand-panel ul a .brand-link-name').allTextContents(), expected);
      assert.ok(await dropdown.getByRole('link', { name: 'Carta e forme', exact: true }).isVisible());
      if (viewport.width > 800) {
        const boxes = await page.locator('.category-menu').evaluate(menu => ({ previousRight: menu.lastElementChild.previousElementSibling.getBoundingClientRect().right, brandLeft: menu.lastElementChild.getBoundingClientRect().left }));
        assert.ok(Math.abs(boxes.brandLeft - boxes.previousRight - 8) < 1, 'Marchi follows the last category with the normal menu gap');
      }
      if (js) {
        await page.keyboard.press('Escape');
        assert.equal(await dropdown.evaluate(el => el.open), false);
        assert.equal(await summary.evaluate(el => el === document.activeElement), true);
        await openMenu();
        await page.getByLabel('Cerca un prodotto', { exact: true }).click();
        assert.equal(await dropdown.evaluate(el => el.open), false);
      } else {
        await summary.focus(); await page.keyboard.press('Enter');
        assert.equal(await dropdown.evaluate(el => el.open), false);
      }
    }
    await page.goto(origin + '/catalog');
    await openMenu();
    await page.screenshot({ path: path.join(output, `brand-menu-${name}.png`) });
    const link = dropdown.getByRole('link', { name: 'Carta e forme', exact: true });
    await link.focus(); await page.keyboard.press('Enter');
    await page.waitForURL(route);
    assert.equal(await page.getByRole('heading', { level: 1 }).textContent(), brand.name);
    assert.equal(await page.locator('#store-search-form').getAttribute('action'), '/search');
    assert.equal(await page.locator('.brand-menu-item.is-selected').count(), 1);
    assert.equal(await dropdown.locator('a[aria-current]').getAttribute('href'), '/brands/carta-e-forme');
    assert.equal(await page.getByLabel('Marchio', { exact: true }).count(), 0);
    const categories = page.getByLabel('Categoria', { exact: true });
    const relevant = (await query("WITH RECURSIVE relevant(id,parent_id) AS (SELECT DISTINCT c.id,c.parent_id FROM catalog_visible_products p JOIN catalog_brands b ON b.id=p.brand_id JOIN catalog_categories c ON c.id=p.category_id WHERE b.slug='carta-e-forme' UNION SELECT c.id,c.parent_id FROM catalog_categories c JOIN relevant child ON child.parent_id=c.id) SELECT c.slug FROM catalog_categories c WHERE c.active AND c.id IN (SELECT id FROM relevant) ORDER BY c.slug")).rows.map(r => r.slug);
    assert.deepEqual((await categories.locator('option').evaluateAll(options => options.map(o => o.value))).filter(Boolean).sort(), relevant);
    for (const width of [320, 390, 768, 1440]) {
      await page.setViewportSize({ width, height: 1050 }); await page.goto(route);
      await require('./browser-catalog-layout.cjs').check(page);
      await page.locator('.store-brand-logo').evaluate(img => img.decode());
      const logoBox = await page.locator('.store-brand-logo').boundingBox();
      const titleBox = await page.getByRole('heading', { level: 1 }).boundingBox();
      assert.ok(logoBox.x + logoBox.width < titleBox.x, 'Brand logo precedes the name on the left');
      assert.ok(Math.abs(logoBox.y + logoBox.height / 2 - titleBox.y - titleBox.height / 2) < 1, 'Logo and name share a centered row');
      await require('./browser-product-card.cjs').check(page, false);
    }
    await page.setViewportSize(viewport); await page.goto(route);
    const logo = page.locator('.store-brand-logo');
    assert.equal(await logo.count(), 1);
    await logo.evaluate(img => img.decode());
    assert.ok(await logo.evaluate(img => img.naturalWidth > 0 && img.getBoundingClientRect().width <= 180 && img.getBoundingClientRect().height <= 90));
    await page.screenshot({ path: path.join(output, `brand-store-${name}.png`), fullPage: true });
    const cards = page.locator('.catalog-results .store-product');
    assert.equal(await cards.count(), 21);
    const first = await cards.locator('.store-product-title').allTextContents();
    await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/brands/carta-e-forme' && u.searchParams.get('page') === '2');
    assert.ok((await cards.locator('.store-product-title').allTextContents()).every(t => !first.includes(t)));
    await page.getByLabel('Prodotti per pagina', { exact: true }).selectOption('42');
    if (!js) await page.getByRole('button', { name: 'Applica', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/brands/carta-e-forme' && u.searchParams.get('per_page') === '42' && !u.searchParams.has('page'));
    assert.ok(await cards.locator('a[href="/products/brand-view-25"]').count() > 0, 'Uncategorized brand products remain visible');
    await categories.selectOption('cancelleria');
    await page.getByLabel('Solo disponibili', { exact: true }).check();
    await page.getByLabel('Solo disponibili', { exact: true }).focus(); await page.keyboard.press('Tab');
    const apply = page.getByRole('button', { name: 'Applica filtri', exact: true });
    assert.ok(await apply.evaluate(el => el === document.activeElement && getComputedStyle(el).outlineStyle !== 'none'));
    await page.keyboard.press('Enter');
    await page.waitForURL(u => u.pathname === '/brands/carta-e-forme' && u.searchParams.get('available') === 'true');
    assert.equal(new URL(page.url()).searchParams.get('category'), 'cancelleria');
    assert.equal(new URL(page.url()).pathname, '/brands/carta-e-forme');
    assert.equal(new URL(page.url()).searchParams.get('brand'), null);
    assert.ok(await cards.count() > 0);
    await page.getByRole('link', { name: 'Rimuovi filtri', exact: true }).click(); await page.waitForURL(route + '?per_page=42');
    assert.equal(await categories.inputValue(), '');
    assert.equal(await page.getByLabel('Solo disponibili', { exact: true }).isChecked(), false);
    assert.equal(await page.getByLabel('Cerca un prodotto', { exact: true }).inputValue(), '');
    await page.goto(route + '?category=missing-category');
    assert.equal(await cards.count(), 0);
    assert.ok(await page.getByText('Prova a modificare o rimuovere i filtri di questo marchio.', { exact: true }).isVisible());
    await openMenu(); await dropdown.getByRole('link', { name: 'Tutti i marchi', exact: true }).click(); await page.waitForURL(origin + '/brands');
    assert.equal(await page.locator('.brand-directory-list li').count(), Number((await query('SELECT count(*) FROM catalog_brands WHERE active')).rows[0].count));
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(output, `brand-directory-${name}.png`), fullPage: true });
    assert.equal((await page.context().request.get(origin + '/brands/missing-brand')).status(), 404);
  } finally {
    await page.setViewportSize(viewport);
    await query('DELETE FROM catalog_products WHERE id=ANY($1::bigint[])', [inserted]);
    await query('WITH u AS (UPDATE catalog_brands SET description=$1,logo_key=$3 WHERE id=$2 RETURNING id,description) UPDATE catalog_brands_translations t SET description=u.description FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)', [brand.description, brand.id, brand.logo_key]);
  }
};
