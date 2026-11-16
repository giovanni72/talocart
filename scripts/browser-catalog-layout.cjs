const assert = require('node:assert/strict');
const path = require('node:path');

async function check(page) {
  const layout = await page.locator('.catalog-layout').evaluate(root => {
    const box = e => { const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, right: r.right, bottom: r.bottom }; };
    return { sidebar: box(root.querySelector('.catalog-sidebar')), results: box(root.querySelector('.catalog-results')),
      cards: [...root.querySelectorAll('.store-product')].slice(0, 4).map(box), width: innerWidth };
  });
  if (layout.width > 1000) {
    assert.ok(layout.sidebar.right < layout.results.x, 'Sidebar is to the left of the results');
    assert.ok(Math.abs(layout.sidebar.y - layout.results.y) < 1, 'The two columns align at the top');
  } else {
    assert.ok(layout.sidebar.bottom < layout.results.y, 'Filters precede results on smaller screens');
  }
  if (layout.cards.length) assert.ok(Math.abs(layout.cards[0].y - layout.results.y) < 1, 'First card starts at the top of results');
  if (layout.width > 1000) {
    const heading = await page.locator('.catalog-title-row').evaluate(root => {
      const title = root.querySelector('h1').getBoundingClientRect(), toolbar = root.querySelector('.catalog-toolbar').getBoundingClientRect();
      return { title: { right: title.right, y: title.y, height: title.height }, toolbar: { x: toolbar.x, y: toolbar.y, height: toolbar.height } };
    });
    assert.ok(heading.title.right < heading.toolbar.x, 'Page size is to the right of the title');
    assert.ok(Math.abs(heading.title.y + heading.title.height / 2 - heading.toolbar.y - heading.toolbar.height / 2) < 1, 'Title and page size align vertically');
  }
  const columns = layout.width > 800 ? 3 : layout.width > 500 ? 2 : 1;
  if (layout.cards.length > columns) {
    for (let i = 1; i < columns; i++) {
      assert.ok(Math.abs(layout.cards[i].y - layout.cards[0].y) < 1, `${columns} cards per row`);
      assert.ok(layout.cards[i - 1].right < layout.cards[i].x, 'Cards do not overlap');
    }
    assert.ok(layout.cards[columns].y > layout.cards[0].bottom, 'The next card starts a new row');
  }
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'No horizontal page overflow');
  assert.equal(await page.getByRole('complementary', { name: /^(Filtri (catalogo|categoria|marchio|ricerca)|(Catalogue|Category|Brand|Search) filters)$/ }).count(), 1);
  assert.equal(await page.locator('.catalog-results nav[aria-label="Pagine catalogo"], .catalog-results nav[aria-label="Catalogue pages"]').count(), 1);
  assert.ok(await page.locator('#store-brand, #store-category').first().evaluate(e => e.form.id === 'store-filters-form'));
  assert.ok(await page.locator('input[type="checkbox"][name="available"]').evaluate(e => e.form.id === 'store-filters-form'));
}

module.exports = async ({ page, origin, output, name }) => {
  const viewport = page.viewportSize();
  try {
    for (const width of [320, 390, 768, 1000, 1024, 1280, 1440, 1920]) {
      await page.setViewportSize({ width, height: 1050 });
      await page.goto(origin + '/catalog');
      await check(page);
    }
    await page.setViewportSize(viewport);
    await page.goto(origin + '/catalog');
    await page.locator('.store-product-photo img').first().evaluate(img => img.decode());
    await page.screenshot({ path: path.join(output, `catalog-sidebar-${name}.png`) });
    // Applying sidebar filters preserves the keyword in the path and selected category,
    // resetting pagination with a native GET form even when JavaScript is disabled.
    await page.goto(origin + '/search/Quaderno?category=cancelleria&page=2');
    const brand = page.getByLabel('Marchio', { exact: true });
    await brand.focus();
    await brand.selectOption('carta-e-forme');
    const available = page.getByLabel('Solo disponibili', { exact: true });
    await available.focus();
    await page.keyboard.press('Space');
    await page.keyboard.press('Tab');
    const apply = page.getByRole('button', { name: 'Applica filtri', exact: true });
    assert.ok(await apply.evaluate(e => e === document.activeElement && getComputedStyle(e).outlineStyle !== 'none'));
    await page.keyboard.press('Enter');
    await page.waitForURL(u => u.searchParams.get('brand') === 'carta-e-forme' && u.searchParams.get('available') === 'true');
    const params = new URL(page.url()).searchParams;
    assert.equal(new URL(page.url()).pathname, '/search/Quaderno');
    assert.equal(params.get('category'), 'cancelleria');
    assert.equal(params.get('page'), null);
    assert.equal(await brand.inputValue(), 'carta-e-forme');
    assert.equal(await available.isChecked(), true);
    assert.ok(await page.locator('.catalog-results .store-product').count() > 0);
    const reset = page.getByRole('link', { name: 'Rimuovi filtri', exact: true });
    assert.ok(await reset.evaluate(e => !!e.closest('.catalog-sidebar')));
    await reset.focus(); await page.keyboard.press('Enter');
    await page.waitForURL(origin + '/search/Quaderno');
    assert.equal(await brand.inputValue(), '');
    assert.equal(await available.isChecked(), false);
    await page.goto(origin + '/catalog');
    await page.getByRole('navigation', { name: 'Pagine catalogo' }).getByRole('link', { name: 'Successivo', exact: true }).click();
    await page.waitForURL(u => u.searchParams.get('page') === '2');
    await check(page);
    assert.equal(await page.locator('.catalog-results .store-product').count(), 6);
    await page.goto(origin + '/search/koala-sidebar-zero-results?brand=carta-e-forme&available=true');
    await check(page);
    assert.equal(await page.locator('.catalog-results .store-product').count(), 0);
    assert.equal(await page.locator('.catalog-results').getByRole('heading', { name: 'Nessun prodotto trovato.', exact: true }).count(), 1);
    assert.equal(await brand.inputValue(), 'carta-e-forme');
    assert.equal(await available.isChecked(), true);
    assert.ok(await page.getByRole('button', { name: 'Applica filtri', exact: true }).isVisible());
  } finally {
    await page.setViewportSize(viewport);
  }
};
module.exports.check = check;
