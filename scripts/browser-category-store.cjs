const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, js, output, name }) => {
  await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
  const category = (await query("SELECT id,name,description FROM catalog_categories WHERE slug='cancelleria'")).rows[0];
  const inserted = (await query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) SELECT 'ZZ Categoria test '||n,'CATEGORY-VIEW-'||n,'category-view-'||n,'published',p.category_id,p.brand_id,p.tax_class_id,100,5 FROM (SELECT category_id,brand_id,tax_class_id FROM catalog_visible_products WHERE category_slug='cancelleria' LIMIT 1) p CROSS JOIN generate_series(1,25) n RETURNING id")).rows.map(r => r.id);
  const viewport = page.viewportSize();
  const route = origin + '/catalog/cancelleria';
  try {
    await query('WITH u AS (UPDATE catalog_categories SET description=$1 WHERE id=$2 RETURNING id,description) UPDATE catalog_categories_translations t SET description=u.description FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)', ['Quaderni, idee e strumenti per dare forma ai tuoi progetti.', category.id]);
    for (const width of [320, 390, 768, 1440]) {
      await page.setViewportSize({ width, height: 1050 });
      await page.goto(route);
      await require('./browser-catalog-layout.cjs').check(page);
      assert.equal(await page.getByRole('heading', { level: 1 }).textContent(), category.name);
      assert.ok(await page.getByRole('complementary', { name: 'Filtri categoria', exact: true }).isVisible());
      assert.equal(await page.locator('.category-description').textContent(), 'Quaderni, idee e strumenti per dare forma ai tuoi progetti.');
      assert.equal(await page.getByRole('link', { name: 'Rimuovi filtri', exact: true }).count(), 0);
    }
    await page.setViewportSize(viewport);
    await page.goto(route);
    await page.locator('.store-product-photo img').first().evaluate(img => img.decode());
    await page.screenshot({ path: path.join(output, `category-store-${name}.png`), fullPage: true });
    const cards = page.locator('.catalog-results .store-product');
    assert.equal(await cards.count(), 21);
    const titles = await cards.locator('.store-product-title').allTextContents();
    const next = page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true });
    await next.focus(); await page.keyboard.press('Enter');
    await page.waitForURL(u => u.pathname === '/catalog/cancelleria' && u.searchParams.get('page') === '2');
    assert.ok((await cards.locator('.store-product-title').allTextContents()).every(t => !titles.includes(t)));
    await page.getByLabel('Prodotti per pagina', { exact: true }).selectOption('42');
    if (!js) await page.getByRole('button', { name: 'Applica', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/catalog/cancelleria' && u.searchParams.get('per_page') === '42' && !u.searchParams.has('page'));
    const relevant = (await query("WITH RECURSIVE subtree(id) AS (SELECT id FROM catalog_categories WHERE slug='cancelleria' UNION ALL SELECT c.id FROM catalog_categories c JOIN subtree parent ON c.parent_id=parent.id) SELECT DISTINCT b.slug FROM catalog_visible_products p JOIN catalog_brands b ON b.id=p.brand_id WHERE p.category_id IN (SELECT id FROM subtree) ORDER BY b.slug")).rows.map(r => r.slug);
    const brands = page.getByLabel('Marchio', { exact: true });
    assert.deepEqual((await brands.locator('option').evaluateAll(options => options.map(o => o.value))).filter(Boolean).sort(), relevant);
    await brands.selectOption('carta-e-forme');
    await page.getByLabel('Solo disponibili', { exact: true }).check();
    const apply = page.getByRole('button', { name: 'Applica filtri', exact: true });
    await page.getByLabel('Solo disponibili', { exact: true }).focus();
    await page.keyboard.press('Tab');
    assert.ok(await apply.evaluate(e => e === document.activeElement && getComputedStyle(e).outlineStyle !== 'none'));
    await page.keyboard.press('Enter');
    await page.waitForURL(u => u.pathname === '/catalog/cancelleria' && u.searchParams.get('available') === 'true');
    assert.equal(new URL(page.url()).pathname, '/catalog/cancelleria');
    assert.ok(await cards.count() > 0);
    const reset = page.getByRole('link', { name: 'Rimuovi filtri', exact: true });
    await reset.focus(); await page.keyboard.press('Enter');
    await page.waitForURL(route + '?per_page=42');
    assert.equal(await brands.inputValue(), '');
    assert.equal(await page.getByLabel('Solo disponibili', { exact: true }).isChecked(), false);
    assert.equal(await page.getByLabel('Cerca un prodotto', { exact: true }).inputValue(), '');
    await page.goto(route + '?brand=talocart-atelier');
    assert.equal(await cards.count(), 0);
    assert.ok(await page.getByText('Prova a modificare o rimuovere i filtri di questa categoria.', { exact: true }).isVisible());
    for (const slug of ['missing-category', 'archivio']) {
      assert.equal((await page.context().request.get(origin + '/catalog/' + slug)).status(), 404);
    }
  } finally {
    await page.setViewportSize(viewport);
    await query('DELETE FROM catalog_products WHERE id=ANY($1::bigint[])', [inserted]);
    await query('WITH u AS (UPDATE catalog_categories SET description=$1 WHERE id=$2 RETURNING id,description) UPDATE catalog_categories_translations t SET description=u.description FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)', [category.description, category.id]);
  }
};
