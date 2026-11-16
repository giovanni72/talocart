// Purchase rules use pieces, and packs are absolute multiples above the minimum.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, id, js, output, name }) => {
  const original = (await query('SELECT minimum_quantity,packs,stock FROM catalog_products WHERE id=$1', [id])).rows[0];
  const catalogCard = () => page.locator('.store-product').filter({has:page.locator('a[href="/products/tazza-atelier"]')});
  const editor = `/admin/catalog/products/${id}`;
  const minimum = () => page.getByLabel('Quantità minima acquistabile', {exact:true});
  const packs = () => page.getByLabel('Packs', {exact:true});
  const open = async product => {
    await page.goto(origin + `/admin/catalog/products/${product}?tab=commerce`);
    assert.equal(await minimum().isVisible(), true);
    assert.equal(await packs().isVisible(), true);
  };
  const save = async product => {
    await page.getByRole('button', {name:'Salva prodotto',exact:true}).click();
    await page.waitForURL(`**/admin/catalog/products/${product}?saved=1&tab=commerce`);
    await page.getByRole('status').waitFor();
  };
  await open(id);
  assert.equal(await minimum().inputValue(), '1');
  assert.equal(await packs().inputValue(), '1');
  await minimum().focus(); await page.keyboard.press('Tab');
  assert.ok(await packs().evaluate(el => document.activeElement === el), 'Packs follows the minimum in keyboard order');
  await minimum().fill('10'); await packs().fill('6');
  await page.getByLabel('Quantità disponibile', {exact:true}).fill('11');
  await save(id);
  assert.deepEqual((await query('SELECT minimum_quantity,packs,stock FROM catalog_products WHERE id=$1', [id])).rows[0], {minimum_quantity:10,packs:6,stock:11});
  await page.goto(origin + '/products/tazza-atelier');
  assert.ok((await page.locator('.product-purchase-rules').innerText()).includes('Primo acquisto: 12 pezzi.'));
  assert.equal(await page.locator('.product-availability').innerText(), 'Non disponibile');
  assert.equal(await page.locator('[data-product-price]').innerText(), '14,90 € IVA inclusa', 'Price still counts one piece');
  await page.goto(origin + '/search/MUG?available=true');
  assert.equal(await catalogCard().count(), 0);
  await open(id); await page.getByLabel('Quantità disponibile', {exact:true}).fill('12'); await save(id);
  await page.goto(origin + '/products/tazza-atelier');
  assert.equal(await page.locator('.product-availability').innerText(), 'Disponibile');
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({path:path.join(output, `purchase-quantities-public-${name}.png`),fullPage:true});
  await page.goto(origin + '/search/MUG?available=true'); assert.equal(await catalogCard().count(),1, `Purchasable mug appears in filtered catalog (${name})`);
  await open(id); await minimum().fill('0'); await packs().fill('0');
  await page.locator('form.product-editor').evaluate(form => { form.noValidate = true; });
  const rejected = page.waitForResponse(r => new URL(r.url()).pathname === editor && r.request().method() === 'POST');
  await page.getByRole('button', {name:'Salva prodotto',exact:true}).click(); assert.equal((await rejected).status(),422);
  for (const field of [minimum(), packs()]) {
    assert.equal(await field.isVisible(),true); assert.equal(await field.getAttribute('aria-invalid'),'true');
    assert.ok(await field.getAttribute('aria-describedby'));
  }
  assert.equal(await minimum().inputValue(),'0'); assert.equal(await packs().inputValue(),'0');
  assert.equal(await page.locator('#minimum-quantity-error').isVisible(),true);
  assert.equal(await page.locator('#packs-error').isVisible(),true);
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({path:path.join(output, `purchase-quantities-errors-${name}.png`),fullPage:true});
  await minimum().fill(String(original.minimum_quantity)); await packs().fill(String(original.packs));
  await page.getByLabel('Quantità disponibile', {exact:true}).fill(String(original.stock)); await save(id);
  const model = (await query("SELECT id,minimum_quantity,packs FROM catalog_products WHERE slug='tshirt-atelier'")).rows[0];
  await open(model.id); await packs().fill('6'); await save(model.id);
  await page.goto(origin + '/products/tshirt-atelier');
  for (const [label, option] of [['Colore', 'Blu'], ['Taglia', 'M']]) {
    const select = page.getByLabel(label,{exact:true});
    if (js) {
      await page.evaluate(() => { window.quantitiesSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.quantitiesSettled = true; }, {once:true}); });
      await select.selectOption({label:option});
      await page.waitForFunction(() => window.quantitiesSettled === true);
    } else await select.selectOption({label:option});
  }
  if (!js) { await page.getByRole('button',{name:'Mostra variante',exact:true}).click(); await page.waitForURL('**/products/tshirt-atelier?*'); }
  assert.ok((await page.locator('.product-purchase-rules').innerText()).includes('Packs: 6 pezzi per confezione.'));
  assert.ok((await page.locator('.product-purchase-rules').innerText()).includes('Primo acquisto: 6 pezzi.'));
  assert.ok((await page.locator('[data-product-sku]').innerText()).includes('BLUE-M'));
  await open(model.id); await minimum().fill(String(model.minimum_quantity)); await packs().fill(String(model.packs)); await save(model.id);
};

// Configured parents expose summaries and submit only their shared purchase rules.
module.exports.variantSummary = async ({ page, query, origin, js }) => {
  const product = (await query("SELECT id,price_cents,stock,stock_revision,minimum_quantity,packs FROM catalog_products WHERE slug='tshirt-atelier'")).rows[0];
  const simple = (await query("SELECT id FROM catalog_products WHERE slug='tazza-atelier'")).rows[0];
  const variants = (await query('SELECT id,active,price_cents,stock FROM catalog_variants WHERE product_id=$1 ORDER BY id', [product.id])).rows;
  const theme = (await query('SELECT theme_id FROM storefront_settings WHERE id=1')).rows[0].theme_id;
  const editor = `/admin/catalog/products/${product.id}`;
  const summary = page.locator('[data-variant-commerce-summary]');
  const open = () => page.goto(origin + editor + '?tab=commerce');
  const save = async () => {
    const [response] = await Promise.all([
      page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === editor),
      page.waitForNavigation({waitUntil:'load'}),
      page.locator('[data-product-save] button').click(),
    ]);
    assert.equal(response.status(), 303);
    assert.deepEqual((await query('SELECT price_cents,stock,stock_revision FROM catalog_products WHERE id=$1', [product.id])).rows[0], {price_cents:product.price_cents,stock:product.stock,stock_revision:product.stock_revision});
  };
  try {
    for (const selectedTheme of ['default', 'essential']) {
      await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [selectedTheme]);
      for (const lang of ['it', 'en']) {
        await query('UPDATE administrators SET locale=$1', [lang]);
        await query('UPDATE catalog_variants SET active=false WHERE product_id=$1', [product.id]);
        await query('UPDATE catalog_variants SET active=true,price_cents=2000,stock=7 WHERE id=$1', [variants[0].id]);
        await open();
        assert.equal(await summary.isVisible(), true);
        assert.equal(await page.locator('input[name="price"],input[name="stock"]').count(), 0);
        assert.equal(await summary.locator('[data-variant-price]').innerText(), lang === 'it' ? '20,00 €' : '20.00 €');
        assert.equal(await summary.locator('[data-variant-stock]').innerText(), '7');
        assert.ok((await summary.innerText()).includes(lang === 'it' ? 'gestiti dalle singole varianti' : 'managed by individual variants'));
        const submitted = await page.locator('#product-editor-form').evaluate(form => Object.fromEntries(new FormData(form)));
        assert.equal(Object.hasOwn(submitted, 'price'), false); assert.equal(Object.hasOwn(submitted, 'stock'), false);
        await page.locator('#product-minimum-quantity').fill('2');
        await page.locator('#product-minimum-quantity').focus(); await page.keyboard.press('Tab');
        assert.equal(await page.locator('#product-packs').evaluate(el => el === document.activeElement), true);
        await page.locator('#product-packs').fill('3'); await save();
        assert.deepEqual((await query('SELECT minimum_quantity,packs FROM catalog_products WHERE id=$1', [product.id])).rows[0], {minimum_quantity:2,packs:3});
        await open();
        // An obsolete/forged form cannot replace the parent's conserved values.
        await page.locator('#product-editor-form').evaluate(form => {
          for (const name of ['price', 'stock']) { const input = document.createElement('input'); input.type = 'hidden'; input.name = name; input.value = '0'; form.append(input); }
        });
        await save();
        await query('UPDATE catalog_variants SET active=true,price_cents=3000,stock=0 WHERE id=$1', [variants[1].id]);
        await open();
        assert.equal(await summary.locator('[data-variant-price]').innerText(), lang === 'it' ? '20,00 – 30,00 €' : '20.00 – 30.00 €');
        assert.equal(await summary.locator('[data-variant-stock]').innerText(), '7');
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
        assert.ok(await summary.locator('dt,dd').evaluateAll(nodes => nodes.every(el => el.scrollWidth <= el.clientWidth + 1)), 'Summary labels and values fit their columns');
        await summary.locator('a').focus();
        assert.equal(await summary.locator('a').evaluate(el => el === document.activeElement), true);
        await Promise.all([page.waitForNavigation({waitUntil:'load'}), page.keyboard.press('Enter')]);
        assert.equal(new URL(page.url()).searchParams.get('tab'), 'variants');
        assert.equal(await page.locator('#panel-variants').isVisible(), true);
        if (js) assert.equal(await page.locator('#tab-variants').getAttribute('aria-selected'), 'true');
        await query('UPDATE catalog_variants SET active=false WHERE product_id=$1', [product.id]);
        await open();
        assert.equal(await summary.locator('[data-variant-price]').innerText(), lang === 'it' ? 'Nessuna variante attiva' : 'No active variants');
        assert.equal(await summary.locator('[data-variant-stock]').innerText(), '0');
        // Server validation keeps the summary and submitted purchase rules available.
        await page.locator('#product-minimum-quantity').fill('0');
        await page.locator('#product-editor-form').evaluate(form => { form.noValidate = true; });
        const [invalid] = await Promise.all([page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === editor), page.waitForNavigation({waitUntil:'load'}), page.locator('[data-product-save] button').click()]);
        assert.equal(invalid.status(), 422);
        assert.equal(await summary.isVisible(), true);
        assert.equal(await page.locator('#product-minimum-quantity').getAttribute('aria-invalid'), 'true');
        assert.equal(await page.locator('input[name="price"],input[name="stock"]').count(), 0);
        await page.goto(origin + `/admin/catalog/products/${simple.id}?tab=commerce`);
        for (const field of ['#product-price', '#product-stock']) { assert.equal(await page.locator(field).isEditable(), true); assert.equal(await page.locator(field).isVisible(), true); }
        const converted = (await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Review conversion','REVIEW-CONVERSION','review-conversion',1234,5,'published',tax_class_id FROM catalog_products WHERE id=$1 RETURNING id", [product.id])).rows[0];
        await query('INSERT INTO catalog_product_options(product_id,position,option_id) SELECT $1,1,option_id FROM catalog_product_options WHERE product_id=$2 AND position=1', [converted.id,product.id]);
        const conversionPath = `/admin/catalog/products/${converted.id}`;
        await page.goto(origin + conversionPath + '?tab=variants');
        for (const field of ['option_1','option_2','option_3']) await page.locator(`[name="${field}"]`).selectOption('');
        await Promise.all([page.waitForNavigation({waitUntil:'load'}),page.locator(`form[action="${conversionPath}/options"] button[type=submit]`).click()]);
        assert.deepEqual((await query('SELECT status,price_cents,stock FROM catalog_products WHERE id=$1', [converted.id])).rows[0], {status:'draft',price_cents:'1234',stock:5});
        assert.equal((await query('SELECT count(*)::int AS n FROM catalog_visible_products WHERE id=$1', [converted.id])).rows[0].n, 0);
        await page.goto(origin + conversionPath + '?tab=commerce');
        assert.equal(await page.locator('#product-price').inputValue(), '12,34');
        assert.equal(await page.locator('#product-stock').inputValue(), '5');
        await page.locator('#product-price').fill('25'); await page.locator('#product-stock').fill('8');
        await page.locator('#tab-organization').click();
        await page.locator('#product-status').selectOption('published');
        await Promise.all([page.waitForNavigation({waitUntil:'load'}), page.locator('[data-product-save] button').click()]);
        assert.deepEqual((await query('SELECT price_cents,stock FROM catalog_visible_products WHERE id=$1', [converted.id])).rows[0], {price_cents:'2500',stock:8});
        await query('DELETE FROM catalog_products WHERE id=$1', [converted.id]);

      }
    }
  } finally {
    for (const variant of variants) await query('UPDATE catalog_variants SET active=$2,price_cents=$3,stock=$4 WHERE id=$1', [variant.id,variant.active,variant.price_cents,variant.stock]);
    await query('UPDATE catalog_products SET minimum_quantity=$2,packs=$3 WHERE id=$1', [product.id,product.minimum_quantity,product.packs]);
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [theme]);
    await query("UPDATE administrators SET locale='it'");
  }
};

// Stock control is shared by simple products and every active variant.
module.exports.stockPolicy = async ({ page, query, origin, js }) => {
  const products = (await query("SELECT id,slug,stock,manage_stock FROM catalog_products WHERE slug IN ('tazza-atelier','tshirt-atelier') ORDER BY slug")).rows;
  const variants = (await query('SELECT id,product_id,stock,active FROM catalog_variants ORDER BY id')).rows;
  const theme = (await query('SELECT theme_id FROM storefront_settings WHERE id=1')).rows[0].theme_id;
  const save = async product => {
    const [response] = await Promise.all([
      page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === `/admin/catalog/products/${product.id}`),
      page.waitForNavigation({waitUntil:'load'}),
      page.locator('[data-product-save] button').click(),
    ]);
    assert.equal(response.status(), 303);
  };
  try {
    for (const selectedTheme of ['default', 'essential']) {
      await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [selectedTheme]);
      for (const lang of ['it', 'en']) {
        await query('UPDATE administrators SET locale=$1', [lang]);
        for (const product of products) {
          const configured = product.slug === 'tshirt-atelier';
          const label = lang === 'it' ? 'Gestisci scorte' : 'Manage stock';
          const unmanaged = lang === 'it' ? 'Scorte non gestite' : 'Stock not managed';
          await query('UPDATE catalog_products SET manage_stock=true,stock=0 WHERE id=$1', [product.id]);
          await query('UPDATE catalog_variants SET stock=0 WHERE product_id=$1', [product.id]);
          await page.goto(origin + `/admin/catalog/products/${product.id}?tab=commerce`);
          const checkbox = page.getByRole('checkbox', {name:label,exact:true});
          assert.equal(await checkbox.isChecked(), true);
          await checkbox.focus(); await page.keyboard.press('Space');
          assert.equal(await checkbox.isChecked(), false);
          if (!configured && js) assert.equal(await page.locator('#product-stock').isDisabled(), true);
          await save(product);
          assert.deepEqual((await query('SELECT manage_stock,stock FROM catalog_products WHERE id=$1', [product.id])).rows[0], {manage_stock:false,stock:0});
          if (configured) assert.equal(await page.locator('[data-variant-stock]').innerText(), unmanaged);
          else assert.equal(await page.locator('#product-stock').isDisabled(), true);
          assert.ok((await page.locator('[data-stock-unmanaged]').innerText()).includes(unmanaged));
          assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
          const prefix = lang === 'en' ? '/en' : '';
          await page.goto(origin + prefix + '/search/' + (configured ? 'SHIRT' : 'MUG') + '?available=true');
          assert.equal(await page.locator(`.store-product:has(a[href="${prefix}/products/${product.slug}"])`).count(), 1);
          let productPath = prefix + `/products/${product.slug}`;
          if (configured) {
            const picks = (await query('SELECT cv.value_id FROM catalog_variant_values cv JOIN catalog_variants v ON v.id=cv.variant_id JOIN catalog_option_values x ON x.id=cv.value_id JOIN catalog_product_options a ON a.product_id=v.product_id AND a.option_id=x.option_id WHERE v.id=(SELECT id FROM catalog_variants WHERE product_id=$1 AND active ORDER BY id LIMIT 1) ORDER BY a.position', [product.id])).rows;
            productPath += '?' + picks.map((row,i) => `value_${i+1}=${row.value_id}`).join('&');
          }
          await page.goto(origin + productPath);
          assert.equal(await page.locator('.product-availability').innerText(), lang === 'it' ? 'Disponibile' : 'Available');
          assert.equal(await page.locator('#cart-quantity').getAttribute('max'), '999999');
          assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
          await page.goto(origin + `/admin/catalog/products/${product.id}?tab=commerce`);
          await page.locator('#product-manage-stock').check();
          await save(product);
          assert.equal((await query('SELECT manage_stock FROM catalog_products WHERE id=$1', [product.id])).rows[0].manage_stock, true);
          await page.goto(origin + productPath);
          assert.equal(await page.locator('.product-availability').innerText(), lang === 'it' ? 'Non disponibile' : 'Unavailable');
        }
      }
    }
  } finally {
    for (const product of products) await query('UPDATE catalog_products SET manage_stock=$1,stock=$2 WHERE id=$3', [product.manage_stock,product.stock,product.id]);
    for (const variant of variants) await query('UPDATE catalog_variants SET stock=$1,active=$2 WHERE id=$3', [variant.stock,variant.active,variant.id]);
    await query('UPDATE administrators SET locale=\'it\'');
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1', [theme]);
  }
};
