const assert = require('node:assert/strict');
const path = require('node:path');

// Synthetic subtree in the caller's dedicated schema; the parent has no products.
module.exports = async ({ page, query, origin, js, output, name }) => {
  const viewport = page.viewportSize();
  const categories = [];
  let inserted = [];
  const create = async (label, slug, parent = null) => {
    const id = (await query('INSERT INTO catalog_categories(name,slug,parent_id) VALUES ($1,$2,$3) RETURNING id', [label, slug, parent])).rows[0].id;
    categories.push(id);
    return id;
  };
  try {
    const root = await create('Parent descendants', 'descendants-parent');
    const child = await create('Child descendants', 'descendants-child', root);
    const grandchild = await create('Grandchild descendants', 'descendants-grandchild', child);
    const refs = (await query("SELECT tax_class_id FROM catalog_visible_products LIMIT 1")).rows[0];
    const brand = (await query("SELECT id FROM catalog_brands WHERE slug='carta-e-forme'")).rows[0].id;
    const secondBrand = (await query("SELECT id FROM catalog_brands WHERE slug='talocart-atelier'")).rows[0].id;
    inserted = (await query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock,minimum_quantity,packs) SELECT 'Descendants '||lpad(n::text,2,'0'),'DESCENDANTS-'||n,'descendants-'||n,'published',CASE WHEN n<=22 THEN $1::bigint ELSE $2::bigint END,CASE WHEN n<=22 THEN $3::bigint ELSE $4::bigint END,$5,100,CASE WHEN n%2=0 THEN 3 ELSE 5 END,3,2 FROM generate_series(1,25) n RETURNING id", [child, grandchild, brand, secondBrand, refs.tax_class_id])).rows.map(r => r.id);
    const assignment = (await query('SELECT id,category_id FROM catalog_products WHERE id=ANY($1::bigint[]) ORDER BY id', [inserted])).rows;
    const cards = page.locator('.catalog-results .store-product');
    const titles = () => cards.locator('.store-product-title a').allTextContents();
    for (const [prefix, locale, labels] of [
      ['', 'it', { count: '25 prodotti', next: 'Successivo', apply: 'Applica', filters: 'Applica filtri', reset: 'Rimuovi filtri', search: 'Cerca' }],
      ['/en', 'en', { count: '25 products', next: 'Next', apply: 'Apply', filters: 'Apply filters', reset: 'Remove filters', search: 'Search' }],
    ]) {
      await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
      const route = `${origin}${prefix}/catalog/descendants-parent`;
      for (const width of [320, 390, 768, 1440]) {
        await page.setViewportSize({ width, height: 1050 });
        const response = await page.goto(route);
        assert.equal(response.status(), 200);
        assert.equal(response.headers()['content-language'], locale);
        assert.equal(await page.locator('.catalog-result-count').textContent(), labels.count);
        assert.equal(await cards.count(), 21);
        await require('./browser-catalog-layout.cjs').check(page);
      }
      await page.setViewportSize(viewport);
      const firstTitles = await titles();
      await page.getByRole('link', { name: labels.next, exact: true }).focus();
      await page.keyboard.press('Enter');
      await page.waitForURL(u => u.pathname === `${prefix}/catalog/descendants-parent` && u.searchParams.get('page') === '2');
      assert.equal(await cards.count(), 4);
      assert.equal(new Set([...firstTitles, ...await titles()]).size, 25);
      assert.equal(await page.locator('.catalog-result-count').textContent(), labels.count);
      await page.locator('#catalog-page-size').selectOption('42');
      if (!js) await page.getByRole('button', { name: labels.apply, exact: true }).click();
      await page.waitForURL(u => u.searchParams.get('per_page') === '42' && !u.searchParams.has('page'));
      assert.equal(await cards.count(), 25);
      assert.deepEqual((await page.locator('#store-brand option').evaluateAll(options => options.map(o => o.value))).filter(Boolean).sort(), ['carta-e-forme', 'talocart-atelier']);
      await page.screenshot({ path: path.join(output, `category-descendants-${locale}-${name}.png`), fullPage: true });
      await page.screenshot({ path: path.join(output, `category-descendants-${locale}-${name}-viewport.png`) });
      await page.locator('#store-brand').selectOption('talocart-atelier');
      await page.locator('input[type="checkbox"][name="available"]').check();
      await page.getByRole('button', { name: labels.filters, exact: true }).focus();
      await page.keyboard.press('Enter');
      await page.waitForURL(u => u.searchParams.get('brand') === 'talocart-atelier' && u.searchParams.get('available') === 'true');
      assert.equal(await cards.count(), 2);
      assert.deepEqual(await titles(), ['Descendants 23', 'Descendants 25']);
      await page.getByRole('link', { name: labels.reset, exact: true }).click();
      await page.waitForURL(route + '?per_page=42');
      assert.equal(await cards.count(), 25);
      await page.getByRole('searchbox').fill('Descendants');
      await page.getByRole('button', { name: labels.search, exact: true }).click();
      await page.waitForURL(u => u.pathname === `${prefix}/search/Descendants`);
      assert.equal(new URL(page.url()).searchParams.get('category'), 'descendants-parent');
      assert.equal(await page.locator('#store-category').inputValue(), 'descendants-parent');
      assert.equal(await cards.count(), 25);
      await page.goto(`${origin}${prefix}/brands/carta-e-forme?category=descendants-parent&per_page=42`);
      assert.equal(await page.locator('#store-category option[value="descendants-parent"]').textContent(), 'Parent descendants');
      assert.equal(await cards.count(), 22);
      await page.goto(`${origin}${prefix}/catalog/descendants-grandchild?per_page=42`);
      assert.equal(await cards.count(), 3);
      await page.goto(route + '?brand=missing-brand');
      assert.equal(await cards.count(), 0);
      assert.ok(await page.locator('.store-empty').isVisible());
    }
    assert.deepEqual((await query('SELECT id,category_id FROM catalog_products WHERE id=ANY($1::bigint[]) ORDER BY id', [inserted])).rows, assignment);
  } finally {
    await page.setViewportSize(viewport);
    await query('DELETE FROM catalog_products WHERE id=ANY($1::bigint[])', [inserted]);
    for (const id of categories.reverse()) await query('DELETE FROM catalog_categories WHERE id=$1', [id]);
    await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
  }
};
