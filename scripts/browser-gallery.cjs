const presentation = require('./theme-presentation-helper.cjs');
// Called by the isolated demo suite, already part of CI. No application database is used.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, id, js, output, name }) => {
  const admin = `/admin/catalog/products/${id}?tab=image`;
  const rows = async () => (await query(`SELECT id,image_key,position FROM catalog_product_images WHERE product_id=${id} ORDER BY position`)).rows;
  const main = async () => (await query(`SELECT image_key FROM catalog_products WHERE id=${id}`)).rows[0].image_key;
  const original = await main();
  const cards = page.locator('.gallery-card');
  const upload = async file => {
    await page.goto(origin + admin);
    return require('./media-picker-helper.cjs')(page, 'Seleziona immagini per la galleria', file);
  };
  assert.equal(await upload('crates/web/tests/fixtures/webp-animated.webp'), 422);
  assert.equal((await rows()).length, 0);
  assert.equal(await upload('crates/web/tests/fixtures/webp-lossless.webp'), 303);
  assert.equal(await upload('crates/web/tests/fixtures/logo.png'), 303);
  await cards.last().waitFor(); assert.equal(await cards.count(), 2);
  await require('./ui-contract.cjs')(page);
  const added = await rows();
  const action = async (index, label) => {
    await cards.nth(index).getByRole('button', { name: label, exact: true }).click();
    await page.waitForURL('**/admin/catalog/products/' + id + '?saved=1&tab=image');
    await page.getByText('Prodotto salvato.', { exact: true }).waitFor();
  };
  await action(1, 'Sposta prima immagine 2');
  assert.deepEqual((await rows()).map(r => r.id), [added[1].id, added[0].id]);
  await action(0, 'Usa come principale'); assert.equal(await main(), added[1].image_key);
  assert.equal((await rows())[0].image_key, original);
  await action(0, 'Usa come principale'); assert.equal(await main(), original);
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  { const shot = { path: path.join(output, `gallery-admin-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
  await page.goto(origin + '/products/tazza-atelier');
  const photo = page.getByRole('link', { name: 'Immagine aggiuntiva 2', exact: true });
  await photo.focus(); await page.keyboard.press('Enter');
  await page.waitForURL(url => url.searchParams.get('photo') === String(added[0].id));
  await page.locator(`#product-photo-${added[0].id}[aria-current]`).waitFor();
  assert.equal(await page.locator('.store-detail-photo img').getAttribute('src'), '/media/image/' + (await query('SELECT id FROM media_objects WHERE key=$1',[added[0].image_key])).rows[0].id + '/detail');
  if (js) assert.equal(await page.locator(`#product-photo-${added[0].id}`).evaluate(e => e === document.activeElement), true, 'Photo focus survives HTMX swap');
  assert.ok(await page.locator('.store-detail-photo img').evaluate(img => img.complete && img.naturalWidth > 0));
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  { const shot = { path: path.join(output, `gallery-store-${name}.png`), fullPage: true }; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
  await page.reload(); assert.equal(await page.locator('.store-detail-photo img').getAttribute('src'), '/media/image/' + (await query('SELECT id FROM media_objects WHERE key=$1',[added[0].image_key])).rows[0].id + '/detail');
  await page.goto(origin + admin); await action(1, 'Rimuovi dalla galleria');
  assert.equal((await rows()).length, 1); assert.equal((await rows())[0].position, 1);
  await action(0, 'Rimuovi dalla galleria'); assert.equal((await rows()).length, 0);
  assert.equal(await main(), original);
  await page.getByText('Nessuna immagine aggiuntiva.', { exact: true }).waitFor();
};

async function htmxAction(page, js, action) {
  if (js) await page.evaluate(() => {
    window.gallerySettled = false;
    document.body.addEventListener('htmx:afterSettle', () => { window.gallerySettled = true; }, { once: true });
  });
  await action();
  if (js) await page.waitForFunction(() => window.gallerySettled === true);
}

async function photoAction(page, js, action) {
  if (js) await page.evaluate(() => {
    window.photoChanged = false;
    document.addEventListener('talocart:photo', () => { window.photoChanged = true; }, { once: true });
  });
  await action();
  if (js) await page.waitForFunction(() => window.photoChanged);
}

module.exports.variant = async ({ page, query, origin, js }) => {
  let step='setup';
  try {
  const selection = new URL(page.url());
  const variantPhoto = await page.locator('.store-detail-photo img').getAttribute('src');
  const shirt = (await query("SELECT id,image_key FROM catalog_products WHERE model='SHIRT'")).rows[0];
  const row = (await query(`INSERT INTO catalog_product_images(product_id,image_key,position) SELECT ${shirt.id},image_key,1 FROM catalog_products WHERE model='MUG' RETURNING id,image_key`)).rows[0];
  await page.reload();
  await photoAction(page, js, () => page.getByRole('link', { name: 'Immagine aggiuntiva 1', exact: true }).click());
  await page.waitForURL(url => url.searchParams.get('photo') === String(row.id));
  await page.locator(`#product-photo-${row.id}[aria-current]`).waitFor();
  assert.equal(new URL(page.url()).searchParams.get('value_1'), selection.searchParams.get('value_1'));
  assert.equal(new URL(page.url()).searchParams.get('value_2'), selection.searchParams.get('value_2'));
  assert.equal(await page.locator('[data-product-sku]').innerText(), 'SHIRT-BLUE-M');
  assert.equal(await page.locator('[data-product-price]').innerText(), '26,90 € IVA inclusa');
  assert.equal(await page.locator('.store-detail-photo img').getAttribute('src'), '/media/image/' + (await query('SELECT id FROM media_objects WHERE key=$1',[row.image_key])).rows[0].id + '/detail');
  step='return to variant photo';
  await photoAction(page, js, () => page.getByRole('link', { name: 'Foto del prodotto selezionato', exact: true }).click());
  await page.waitForURL(url => !url.searchParams.get('photo'));
  await page.locator('#product-photo-default[aria-current]').waitFor();
  assert.equal(await page.locator('.store-detail-photo img').getAttribute('src'), variantPhoto);
  step='change variant';
  // Changing the variant resets the selected gallery photo through the existing form.
  await photoAction(page, js, () => page.getByRole('link', { name: 'Immagine aggiuntiva 1', exact: true }).click());
  await page.waitForURL(url => url.searchParams.get('photo') === String(row.id));
  await htmxAction(page, js, () => page.getByLabel('Colore', { exact: true }).selectOption({ label: 'Rosso' }));
  if (!js) await page.getByRole('button', { name: 'Mostra variante', exact: true }).click();
  await page.waitForURL(url => !url.searchParams.has('photo'));
  await htmxAction(page, js, () => page.getByLabel('Taglia', { exact: true }).selectOption({ label: 'S' }));
  if (!js) await page.getByRole('button', { name: 'Mostra variante', exact: true }).click();
  await page.locator('[data-product-sku]').filter({ hasText: 'SHIRT-RED-S' }).waitFor();
  assert.equal(await page.locator('.store-detail-photo img').getAttribute('src'), '/media/image/' + (await query("SELECT m.id FROM catalog_variants v JOIN media_objects m ON m.key=v.image_key WHERE v.product_id=$1 AND v.image_key LIKE '%shirt-red%' ",[shirt.id])).rows[0].id + '/detail');
  if (js) {
    step='history across variant replacement';
    await page.goBack();
    await page.waitForURL(url => url.searchParams.get('value_2') === selection.searchParams.get('value_2'));
    await page.goBack();
    await page.waitForURL(url => url.searchParams.get('photo') === String(row.id));
    await page.locator('[data-product-sku]').filter({hasText: 'SHIRT-BLUE-M'}).waitFor();
    await page.locator(`#product-photo-${row.id}[aria-current]`).waitFor();
  }
  await query(`DELETE FROM catalog_product_images WHERE id=${row.id}`);
  } catch(error) {
    throw new Error(`Gallery variant step ${step}, URL ${page.url()}: ${error.message}`);
  }
};

// Focused storefront matrix: real browser requests, native fallback, live form
// preservation, local history, localized headers and the running build identity.
module.exports.images = async ({ page, query, origin, js, output, name }) => {
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  const product = (await query("SELECT id,brand_id,image_key FROM catalog_products WHERE model='MUG'")).rows[0];
  const mug = product.id;
  const brandLogo = (await query('SELECT logo_key FROM catalog_brands WHERE id=$1', [product.brand_id])).rows[0].logo_key;
  await query('UPDATE catalog_brands SET logo_key=$1 WHERE id=$2', [product.image_key, product.brand_id]);
  const extra = (await query("INSERT INTO catalog_product_images(product_id,image_key,position) SELECT $1,image_key,1 FROM catalog_products WHERE model='SHIRT' RETURNING id", [mug])).rows[0].id;
  try {
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      const prefix = locale === 'it' ? '' : '/en';
      await page.goto(origin + prefix + '/products/tazza-atelier');
      const image = page.locator('.store-detail-photo img');
      await image.evaluate(img => img.decode());
      const build = await page.locator('.store-build-version').innerText();
      assert.match(build, locale === 'it' ? /^Versione build \S+$/ : /^Build version \S+$/);
      const footerBox = await page.locator('.store-build-version').boundingBox();
      assert.ok(footerBox.width > 0 && footerBox.height > 0);
      assert.ok(await page.locator('.store-build-version').evaluate(el => parseFloat(getComputedStyle(el).fontSize) < 12));
      const menuLogo = page.locator('.brand-dropdown img').first();
      assert.match(await menuLogo.getAttribute('src'), /\/thumbnail$/);
      const smallLogo = await page.request.get(origin + await menuLogo.getAttribute('src'), {maxRedirects: 0});
      assert.equal(smallLogo.status(), 200, 'Menu logo is served directly');
      const source = await image.getAttribute('src');
      const direct = await page.request.get(origin + source, {maxRedirects: 0});
      assert.equal(direct.status(), 200, 'Main image is served directly');
      assert.equal(direct.headers()['cache-control'], 'public, max-age=0, must-revalidate');
      const cached = await page.request.get(origin + source, {headers: {'If-None-Match': direct.headers().etag}, maxRedirects: 0});
      assert.equal(cached.status(), 304, 'Browser HTTP cache can revalidate the representation');
      const quantity = page.locator('#cart-quantity');
      if (js && await quantity.getAttribute('hx-post')) {
        const priced = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/products/tazza-atelier'));
        await quantity.fill('2');
        assert.equal((await priced).status(), 200);
        await page.locator('#product-unit-price:not([hidden])').waitFor();
      } else await quantity.fill('2');
      if (js) await quantity.evaluate(el => { window.galleryQuantity = el; window.galleryPurchase = el.form; });
      const requests = [];
      const observe = request => { if (request.method() === 'GET' && new URL(request.url()).pathname.includes('/products/')) requests.push(request.url()); };
      page.on('request', observe);
      try {
        const photo = page.locator(`#product-photo-${extra}`);
        await photo.focus(); await page.keyboard.press('Enter');
        await page.waitForURL(url => url.searchParams.get('photo') === String(extra));
        await page.locator(`#product-photo-${extra}[aria-current]`).waitFor();
        await image.evaluate(img => img.decode());
        const srcset = await image.getAttribute('srcset');
        if (js) {
          assert.deepEqual(requests, [], 'Changing photo never requests/re-renders the product');
          assert.equal(await quantity.inputValue(), '2');
          assert.ok(await quantity.evaluate(el => el === window.galleryQuantity && el.form === window.galleryPurchase));
          assert.ok(await photo.evaluate(el => el === document.activeElement), 'Keyboard focus stays on the chosen photo');
          await page.goBack();
          await page.locator('#product-photo-default[aria-current]').waitFor();
          assert.equal(await image.getAttribute('src'), source);
          await page.goForward();
          await page.locator(`#product-photo-${extra}[aria-current]`).waitFor();
          assert.deepEqual(requests, [], 'Back/Forward also change only the photo');
          assert.equal(await quantity.inputValue(), '2');
        } else assert.ok(requests.length > 0, 'Native photo link navigates without JavaScript');
        const current = new URL(page.url());
        const language = page.locator(`.store-languages a[hreflang="${locale === 'it' ? 'en' : 'it'}"]`);
        assert.equal(new URL(await language.getAttribute('href'), origin).search, current.search);
        await page.reload();
        await page.locator(`#product-photo-${extra}[aria-current]`).waitFor();
        assert.equal(await image.getAttribute('srcset'), srcset, 'Reload preserves the selected image dimensions and responsive candidates');
        await image.evaluate(img => img.decode());
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        const shot = {path: path.join(output, `store-images-${name}-${theme}-${locale}.png`), fullPage: true};
        await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot);
        await page.locator('.store-footer-bottom').screenshot({path: path.join(output, `store-build-${name}-${theme}-${locale}.png`)});
        await page.goto(origin + prefix + '/catalog');
        const cardLogo = page.locator('.store-product-brand-logo').first();
        assert.match(await cardLogo.getAttribute('src'), /\/thumbnail$/);
        assert.match(await cardLogo.getAttribute('srcset'), /\/card [\d.]+x$/);
        await cardLogo.scrollIntoViewIfNeeded();
        await cardLogo.evaluate(img => img.decode());
        const logoBox = await cardLogo.boundingBox();
        assert.ok(logoBox.width <= 120 && logoBox.height <= 56);
      } finally { page.off('request', observe); }
    }
    await page.goto(origin + '/admin/media');
    const upload = await page.request.post(origin + '/admin/media/upload', {
      maxRedirects: 0,
      multipart: {
        csrf: await page.locator('input[name="csrf"]').first().inputValue(),
        images: {name: 'retina-logo.png', mimeType: 'image/png', buffer: require('node:fs').readFileSync('crates/web/tests/fixtures/avatar.png')},
      },
    });
    assert.equal(upload.status(), 303);
    const smallId = Number(new URL(upload.headers().location, origin).searchParams.get('selected'));
    const small = (await query('SELECT key FROM media_objects WHERE id=$1', [smallId])).rows[0];
    await query('UPDATE catalog_brands SET logo_key=$1 WHERE id=$2', [small.key, product.brand_id]);
    const retina = await page.context().browser().newContext({viewport: page.viewportSize(), deviceScaleFactor: 2, javaScriptEnabled: js});
    try {
      const highDensity = await retina.newPage();
      await highDensity.goto(origin + '/catalog');
      const logo = highDensity.locator(`.store-product-brand-logo[src="/media/image/${smallId}/thumbnail"]`).first();
      await logo.scrollIntoViewIfNeeded(); await logo.evaluate(img => img.decode());
      const box = await logo.boundingBox();
      assert.equal(box.width, 32, 'A 32px logo keeps its native size on a Retina screen');
      assert.equal(box.height, 32);
    } finally { await retina.close(); }
    await query('UPDATE catalog_brands SET logo_key=$1 WHERE id=$2', [product.image_key, product.brand_id]);
    // Exercise variant changes after local photo history, using the existing
    // regression for SKU, price and gallery selection together.
    await query("UPDATE storefront_settings SET theme_id='default'");
    await page.goto(origin + '/products/tshirt-atelier');
    for (const [label, option] of [['Colore', 'Blu'], ['Taglia', 'M']]) {
      await htmxAction(page, js, () => page.getByLabel(label, {exact: true}).selectOption({label: option}));
      if (!js) {
        await page.getByRole('button', {name: 'Mostra variante', exact: true}).click();
        await page.waitForLoadState();
      }
    }
    await module.exports.variant({page, query, origin, js});
  } finally {
    await query('DELETE FROM catalog_product_images WHERE id=$1', [extra]);
    await query('UPDATE catalog_brands SET logo_key=$1 WHERE id=$2', [brandLogo, product.brand_id]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
  }
};
