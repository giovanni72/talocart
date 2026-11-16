const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, js, output, name }) => {
  await page.context().clearCookies({ name: 'talocart_catalog_page_size' });
  const viewport = page.viewportSize();
  const inserted = (await query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) SELECT 'Ricerca prova '||lpad(n::text,2,'0'),'SEARCH-VIEW-'||n,'search-view-'||n,CASE WHEN n<=60 THEN 'published' ELSE 'draft' END,CASE WHEN n=60 THEN NULL ELSE p.category_id END,p.brand_id,p.tax_class_id,100,CASE WHEN n%2=0 THEN 0 ELSE 5 END FROM (SELECT category_id,brand_id,tax_class_id FROM catalog_visible_products p JOIN catalog_brands b ON b.id=p.brand_id WHERE b.slug='carta-e-forme' AND p.category_slug='cancelleria' LIMIT 1) p CROSS JOIN generate_series(1,61) n RETURNING id")).rows.map(r => r.id);
  const search = page.getByLabel('Cerca un prodotto', { exact: true });
  const cards = page.locator('.catalog-results .store-product');
  const route = origin + '/search/Ricerca';
  const submit = async (term, expectedPath) => {
    await search.fill(term); await search.focus(); await page.keyboard.press('Enter');
    await page.waitForURL(u => decodeURIComponent(u.pathname) === expectedPath);
    assert.equal(await search.inputValue(), term.trim());
    assert.equal(new URL(page.url()).searchParams.has('q'), false);
  };
  try {
    for (const start of ['/', '/catalog/cancelleria', '/brands/carta-e-forme']) {
      await page.goto(origin + start);
      await submit('Ricerca', '/search/Ricerca');
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get('category'), start.startsWith('/catalog/') ? 'cancelleria' : null);
      assert.equal(params.get('brand'), start.startsWith('/brands/') ? 'carta-e-forme' : null);
      assert.equal(await page.getByRole('heading', { level: 1 }).textContent(), 'Risultati per “Ricerca”');
      assert.ok(await cards.count() > 0);
    }
    for (const width of [320,390,768,1440]) {
      await page.setViewportSize({ width, height: 1050 }); await page.goto(route);
      await require('./browser-catalog-layout.cjs').check(page);
      await require('./browser-product-card.cjs').check(page, false);
    }
    await page.setViewportSize(viewport); await page.goto(route);
    await page.screenshot({ path: path.join(output, `search-store-${name}.png`), fullPage: true });
    assert.equal(await cards.count(), 21);
    const first = await cards.locator('.store-product-title').allTextContents();
    await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/search/Ricerca' && u.searchParams.get('page') === '2');
    assert.ok((await cards.locator('.store-product-title').allTextContents()).every(t => !first.includes(t)));
    await submit('SEARCH-VIEW', '/search/SEARCH-VIEW');
    assert.equal(new URL(page.url()).searchParams.has('page'), false, 'A new keyword resets pagination');
    await page.goto(route + '?category=cancelleria&brand=carta-e-forme&available=true&page=2');
    await page.getByLabel('Prodotti per pagina', { exact: true }).selectOption('42');
    if (!js) await page.getByRole('button', { name: 'Applica', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/search/Ricerca' && u.searchParams.get('per_page') === '42' && !u.searchParams.has('page'));
    for (const [key,value] of [['category','cancelleria'],['brand','carta-e-forme'],['available','true']]) assert.equal(new URL(page.url()).searchParams.get(key), value);
    assert.equal(await cards.count(), 30);
    const category = page.getByLabel('Categoria', { exact: true });
    const brand = page.getByLabel('Marchio', { exact: true });
    assert.ok(await category.evaluate(el => el.form.id === 'store-filters-form'));
    assert.ok(await brand.evaluate(el => el.form.id === 'store-filters-form'));
    await category.selectOption('accessori');
    await page.getByRole('button', { name: 'Applica filtri', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/search/Ricerca' && u.searchParams.get('category') === 'accessori');
    assert.equal(await cards.count(), 30, 'Parent category retains matching products from Cancelleria');
    const reset = page.getByRole('link', { name: 'Rimuovi filtri', exact: true });
    await reset.focus(); await page.keyboard.press('Enter'); await page.waitForURL(route + '?per_page=42');
    assert.equal(await category.inputValue(), ''); assert.equal(await brand.inputValue(), '');
    assert.equal(await page.getByLabel('Solo disponibili', { exact: true }).isChecked(), false);
    assert.equal(await search.inputValue(), 'Ricerca');
    assert.equal(await cards.count(), 42);
    await submit('SEARCH-VIEW-60', '/search/SEARCH-VIEW-60');
    assert.equal(await cards.count(), 1);
    assert.ok(await cards.locator('a[href="/products/search-view-60"]').count() > 0);
    const term = 'Caffè & tè / 50% + #?';
    await query("WITH u AS (UPDATE catalog_products SET name=$1 WHERE model='SEARCH-VIEW-1' RETURNING id,name) UPDATE catalog_products_translations t SET name=u.name FROM u WHERE t.entity_id=u.id AND t.locale=(SELECT primary_locale FROM storefront_settings WHERE id=1)", ['Ricerca ' + term]);
    for (const word of [term, '%2F', '.', '..', '<script>alert(1)</script>']) {
      await submit(word, '/search/' + word + (['.','..'].includes(word) ? ' ' : ''));
      assert.equal(await page.getByRole('heading', { level: 1 }).textContent(), `Risultati per “${word}”`);
      assert.equal(await page.locator('script').filter({ hasText: 'alert(1)' }).count(), 0);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      assert.equal(await cards.count(), word === term ? 1 : 0);
    }
    const variant = (await query('SELECT v.sku FROM catalog_variants v JOIN catalog_visible_products p ON p.id=v.product_id WHERE v.active ORDER BY v.id LIMIT 1')).rows[0];
    await submit(variant.sku, '/search/' + variant.sku);
    assert.ok(await cards.count() > 0, 'Search finds an active variant SKU');
    await page.goto(origin + '/'); await submit('   ', '/catalog');
    for (const old of ['/catalog?category=cancelleria','/catalog?q=Quaderno']) assert.equal((await page.context().request.get(origin + old)).status(), 422);
    await page.goto(origin + '/catalog/cancelleria');
    assert.equal(await page.locator('#store-filters-form').getAttribute('action'), '/catalog/cancelleria');
  } finally {
    await page.setViewportSize(viewport);
    await query('DELETE FROM catalog_products WHERE id=ANY($1::bigint[])', [inserted]);
  }
};
