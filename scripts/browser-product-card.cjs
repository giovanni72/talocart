const assert = require('node:assert/strict');
const path = require('node:path');

async function check(page, nested) {
  const cards = page.locator('.store-product');
  assert.ok(await cards.count() > 0, 'The shared product card is rendered');
  for (const card of await cards.all()) {
    const photo = card.locator('.store-product-photo');
    const box = await photo.boundingBox();
    assert.ok(Math.abs(box.width / box.height - 3 / 4) < .002, 'Image frame has a 3:4 aspect ratio');
    const title = card.locator('.store-product-title');
    assert.equal(await title.evaluate(e => e.tagName), nested ? 'H3' : 'H2');
    assert.equal(await photo.getAttribute('href'), await title.locator('a').getAttribute('href'));
    assert.equal(await photo.getAttribute('aria-label'), await title.innerText());
    assert.match((await card.locator('.store-product-price').innerText()).replace(/\s+/g, ' '), /^(Da )?\d[\d.,]* € IVA inclusa$/);
    assert.match(await card.locator('.product-availability').innerText(), /^(Disponibile|Non disponibile)$/);
    await card.scrollIntoViewIfNeeded();
    const amount = await card.locator('.store-product-price > span').boundingBox();
    const tax = await card.locator('.store-product-price small').boundingBox();
    assert.ok(tax.y >= amount.y + amount.height && Math.abs(tax.x - amount.x) < 1, 'VAT is below the price and left aligned');
    const row = await card.locator('.store-product-price-row').boundingBox();
    const availability = await card.locator('.product-availability').boundingBox();
    assert.ok(availability.x + availability.width <= row.x + row.width + 1 && availability.y >= row.y, 'Availability sits on the price base');
    const titleBox = await title.boundingBox();
    assert.ok(row.y >= titleBox.y + titleBox.height, 'Price base follows the name');
    const brandLogo = card.locator('.store-product-brand-logo');
    if (await brandLogo.count()) {
      await brandLogo.evaluate(img => img.decode());
      const logo = await brandLogo.boundingBox();
      const meta = await card.locator('.store-product-meta').boundingBox();
      assert.ok(logo.y + logo.height <= titleBox.y + 1, 'Brand sits above the name');
      assert.ok(Math.abs(logo.x + logo.width - meta.x - meta.width) < 1, 'Brand is right aligned');
      assert.ok(await brandLogo.evaluate(img => img.naturalWidth > 0 && getComputedStyle(img).objectFit === 'contain'));
    }
    const image = photo.locator('img');
    if (await image.count()) {
      await photo.scrollIntoViewIfNeeded();
      await image.evaluate(img => img.decode());
      const geometry = await image.evaluate(img => {
        const r = img.getBoundingClientRect(), p = img.parentElement.getBoundingClientRect();
        return { fit: getComputedStyle(img).objectFit, loaded: img.naturalWidth > 0,
          inside: r.left >= p.left && r.right <= p.right && r.top >= p.top && r.bottom <= p.bottom };
      });
      assert.deepEqual(geometry, { fit: 'contain', loaded: true, inside: true });
      assert.equal(await image.getAttribute('alt'), await title.innerText());
      assert.equal(await image.getAttribute('loading'), 'lazy');
    } else {
      assert.equal(await photo.innerText(), 'Immagine non disponibile');
    }
  }
  const rows = await cards.evaluateAll(items => items.map(e => ({ top: Math.round(e.getBoundingClientRect().top + scrollY),
    base: Math.round(e.querySelector('.store-product-price-row').getBoundingClientRect().bottom - e.getBoundingClientRect().bottom) })));
  for (const card of rows) assert.ok(rows.filter(other => other.top === card.top).every(other => Math.abs(other.base - card.base) <= 1), 'Price bases align across a row');
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Cards fit the viewport');
}

module.exports = async ({ page, query, origin, output, name }) => {
  const viewport = page.viewportSize();
  const { show_featured: featured } = (await query('SELECT show_featured FROM storefront_settings')).rows[0];
  try {
    await query('UPDATE storefront_settings SET show_featured=true');
    for (const route of ['/', '/catalog']) {
      for (const width of [320, 800, 1024, 1440, 1920, viewport.width]) {
        await page.setViewportSize({ width, height: viewport.height });
        await page.goto(origin + route);
        await check(page, route === '/');
        if (route === '/') {
          const cards = await page.locator('.home-featured .store-product').evaluateAll(items => items.map(e => ({ x: e.offsetLeft, y: e.offsetTop })));
          assert.equal(cards.length, 8, 'Homepage contains eight published products');
          const columns = width > 800 ? 4 : width > 500 ? 2 : 1;
          assert.equal(cards.filter(e => e.y === cards[0].y).length, columns, 'Homepage responsive column count');
          assert.equal(new Set(cards.map(e => e.y)).size, 8 / columns, 'Homepage row count');
          if (width > 800) {
            const spacing = await page.locator('.brand-menu-item').evaluate(e => {
              const current = e.getBoundingClientRect(), previous = e.previousElementSibling.getBoundingClientRect();
              return { gap: current.left - previous.right, sameRow: current.top === previous.top };
            });
            assert.ok(spacing.sameRow && Math.abs(spacing.gap - 8) < 1, 'Marchi follows the last category with the normal menu gap');
          }
        }
      }
      const first = page.locator('.store-product').first();
      const link = first.locator('.store-product-photo');
      const href = await link.getAttribute('href');
      // Both independent links are reachable through the native tab sequence.
      await link.focus();
      assert.ok(await link.evaluate(e => e === document.activeElement && getComputedStyle(e).outlineStyle !== 'none'));
      await page.keyboard.press('Tab');
      assert.ok(await first.locator('.store-product-title a').evaluate(e => e === document.activeElement));
      await page.keyboard.press('Enter');
      await page.waitForURL(origin + href);
      await page.locator('.store-detail-copy h1').waitFor();
      await page.goto(origin + route);
      await page.locator('.store-product-photo').first().scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(output, `product-card-${route === '/' ? 'home' : 'catalog'}-${name}.png`), fullPage: true });
      await page.locator('.store-product').first().screenshot({ path: path.join(output, `product-card-closeup-${route === '/' ? 'home' : 'catalog'}-${name}.png`) });
    }
    await page.goto(origin + '/catalog');
    assert.ok(await page.locator('.store-product-photo:not(:has(img))').count() > 0, 'Missing image uses the same frame');
    assert.ok(await page.locator('.store-product .unavailable').count() > 0, 'Zero stock remains visible');
    assert.ok((await page.locator('.store-product-price').allInnerTexts()).some(price => price.startsWith('Da ')), 'Variant price starts with Da');
  } finally {
    await page.setViewportSize(viewport);
    await query('UPDATE storefront_settings SET show_featured=$1', [featured]);
  }
};
module.exports.check = check;
