const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, origin, viewport, output, name }) => {
  for (const route of ['/', '/catalog/cancelleria', '/products/tazza-atelier']) {
    await page.goto(origin + route);
    const search = page.getByRole('search', { name: 'Cerca nel catalogo', exact: true });
    assert.equal(await search.count(), 1);
    assert.equal(await page.locator('input[name=q]').count(), 1);
    assert.equal(await search.isVisible(), true);
    assert.equal(await page.getByLabel('Cerca un prodotto', { exact: true }).isVisible(), true);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    if (viewport.width > 1100) {
      const boxes = await page.locator('.site-header').evaluate(header => {
        const rect = e => { const r = e.getBoundingClientRect(); return { x: r.x, right: r.right, y: r.y, width: r.width }; };
        return { header: rect(header), logo: rect(header.querySelector('.wordmark')), search: rect(header.querySelector('form')), nav: rect(header.querySelector('nav')) };
      });
      assert.ok(Math.abs(boxes.search.x + boxes.search.width / 2 - boxes.header.x - boxes.header.width / 2) < 1, 'Search is centered in the header');
      assert.ok(boxes.logo.right < boxes.search.x && boxes.search.right < boxes.nav.x, 'Logo, search and navigation do not overlap');
    }
    await page.getByLabel('Cerca un prodotto', { exact: true }).fill('Quaderno');
    if (route === '/') await page.keyboard.press('Enter');
    else await search.getByRole('button', { name: 'Cerca', exact: true }).click();
    await page.waitForURL(u => u.pathname === '/search/Quaderno');
    assert.equal(new URL(page.url()).searchParams.get('category'), route.startsWith('/catalog') ? 'cancelleria' : null);
    assert.ok(await page.locator('.store-product').count() > 0);
    assert.equal(await page.getByLabel('Cerca un prodotto', { exact: true }).inputValue(), 'Quaderno');
  }
  await page.getByLabel('Marchio', { exact: true }).selectOption('carta-e-forme');
  await page.getByLabel('Solo disponibili', { exact: true }).check();
  await page.getByRole('button', { name: 'Applica filtri', exact: true }).click();
  await page.waitForURL(u => u.searchParams.get('brand') === 'carta-e-forme' && u.searchParams.get('available') === 'true');
  assert.equal(new URL(page.url()).pathname, '/search/Quaderno');
  await page.goto(origin + '/');
  await page.screenshot({ path: path.join(output, `header-search-${name}.png`) });
};
