const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, origin, output, name, js, viewport }) => {
  await page.goto(origin + '/catalog?available=true&page=2&per_page=21');
  const before = new URL(page.url());
  await page.getByRole('navigation', { name: 'Lingua del negozio' }).getByRole('link', { name: 'English', exact: true }).click();
  await page.waitForURL(url => url.pathname === '/en/catalog');
  assert.equal(new URL(page.url()).search, before.search);
  assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  assert.equal(await page.locator('.store-languages [aria-current]').innerText(), 'English');
  assert.equal(await page.locator('#store-filters-form').getAttribute('action'), '/en/catalog');
  assert.ok((await page.locator('.store-product-title a').evaluateAll(links => links.map(a => a.getAttribute('href')))).every(href => href.startsWith('/en/products/')));
  assert.equal(await page.locator('link[rel="canonical"]').getAttribute('href'), origin + '/en/catalog');
  assert.equal(await page.locator('link[hreflang="it"]').getAttribute('href'), origin + '/catalog');
  await page.getByRole('link', { name: 'Previous', exact: true }).click();
  await page.waitForURL(url => url.pathname === '/en/catalog' && url.searchParams.get('page') === '1');
  await page.getByRole('searchbox').fill('Tazza');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  await page.waitForURL(url => url.pathname === '/en/search/Tazza');
  assert.equal(new URL(page.url()).searchParams.get('available'), 'true');
  assert.ok(await page.getByRole('heading', { name: 'Results for “Tazza”' }).isVisible());
  await page.getByRole('navigation', { name: 'Store language' }).getByRole('link', { name: 'Italiano', exact: true }).focus();
  await page.keyboard.press('Enter');
  await page.waitForURL(url => url.pathname === '/search/Tazza');
  assert.equal(await page.locator('html').getAttribute('lang'), 'it');
  for (const route of ['/en', '/en/catalog/accessori', '/en/brands', '/en/brands/talocart-atelier', '/en/products/tazza-atelier']) {
    const response = await page.goto(origin + route);
    assert.equal(response.status(), 200, route);
    assert.equal(response.headers()['content-language'], 'en');
    assert.equal(await page.locator('html').getAttribute('lang'), 'en');
    assert.equal(await page.locator('.site-header').count(), 1);
    assert.equal(await page.locator('main').count(), 1);
    assert.equal(await page.getByRole('searchbox').getAttribute('placeholder'), 'Search for a product');
    assert.ok((await page.locator('.category-navigation a').evaluateAll(links => links.map(a => a.getAttribute('href')))).every(href => href.startsWith('/en/')));
    assert.ok((await page.locator('img').evaluateAll(images => images.map(img => img.getAttribute('src')))).every(src => !src.startsWith('/en/')));
  }
  assert.equal(await page.locator('[data-product-price]').innerText(), '14.90 € VAT included');
  assert.ok((await page.locator('.product-purchase-rules').innerText()).includes('Minimum purchase quantity: 1 piece.'));
  await page.goto(origin + '/en/products/tshirt-atelier');
  for (const [label, option] of [['Colore', 'Blu'], ['Taglia', 'M']]) {
    if (js) await page.evaluate(() => { window.languageSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.languageSettled = true; }, { once: true }); });
    await page.getByLabel(label, { exact: true }).selectOption({ label: option });
    if (js) await page.waitForFunction(() => window.languageSettled === true);
  }
  if (!js) {
    await page.getByRole('button', { name: 'Show variant', exact: true }).click();
    await page.waitForURL(url => url.pathname === '/en/products/tshirt-atelier' && url.searchParams.has('value_2'));
  }
  assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
  assert.equal(await page.locator('[data-product-price]').innerText(), '26.90 € VAT included');
  const variant = new URL(page.url());
  const thumbnails = page.locator('.store-photo-thumbnails a');
  assert.ok(await thumbnails.count() >= 2);
  await thumbnails.last().click();
  await page.waitForURL(url => Boolean(url.searchParams.get('photo')));
  const photo = new URL(page.url());
  assert.equal(photo.searchParams.get('value_1'), variant.searchParams.get('value_1'));
  assert.ok(photo.searchParams.get('photo'));
  assert.equal(await page.locator('.store-languages a[hreflang="it"]').getAttribute('href'), photo.pathname.slice(3) + photo.search);
  await page.getByRole('navigation', { name: 'Store language' }).getByRole('link', { name: 'Italiano', exact: true }).click();
  await page.waitForURL(url => url.pathname === '/products/tshirt-atelier');
  assert.equal(new URL(page.url()).search, photo.search);
  assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
  assert.equal(await page.locator('[data-product-price]').innerText(), '26,90 € IVA inclusa');
  await page.goto(origin + '/en/catalog');
  for (const width of [320, 390, 800, 1100, 1440, 1920]) {
    await page.setViewportSize({ width, height: 1050 });
    if (js) await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `English layout at ${width}px`);
    assert.ok(await page.locator('.store-languages').isVisible());
  }
  await page.setViewportSize(viewport);
  await page.goto(origin + '/en/catalog');
  await page.screenshot({ path: path.join(output, `store-language-${name}.png`), fullPage: false });
  await page.getByRole('navigation', { name: 'Store language' }).getByRole('link', { name: 'Italiano', exact: true }).click();
  await page.waitForURL(url => url.pathname === '/catalog');
  assert.equal(await page.locator('html').getAttribute('lang'), 'it');
};
