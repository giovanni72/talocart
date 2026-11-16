// Runs only inside browser-demo's dedicated schema and authenticated context.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, js, output, name }) => {
  await query("INSERT INTO catalog_options(name) SELECT 'ZZ Choice '||lpad(n::text,3,'0') FROM generate_series(1,100)n");
  await page.goto(origin + '/admin/catalog/options/new');
  await page.getByLabel('Nome dell’opzione', { exact: true }).fill('ZZZ Colore esteso');
  await page.getByRole('button', { name: 'Crea opzione', exact: true }).click();
  await page.waitForURL(url => /^\/admin\/catalog\/options\/\d+$/.test(url.pathname));
  const color = new URL(page.url()).pathname.split('/').pop();
  await query("INSERT INTO catalog_options(name) VALUES('ZZZ Taglia estesa'),('ZZZ Materiale esteso')");
  const size = (await query("SELECT id FROM catalog_options WHERE name='ZZZ Taglia estesa'")).rows[0].id;
  const material = (await query("SELECT id FROM catalog_options WHERE name='ZZZ Materiale esteso'")).rows[0].id;
  await query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Colore '||lpad(n::text,3,'0') FROM generate_series(1,501)n", [color]);
  await query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Taglia unica'),($2,'Materiale unico')", [size, material]);
  const sizeValue = (await query('SELECT id FROM catalog_option_values WHERE option_id=$1', [size])).rows[0].id;
  const materialValue = (await query('SELECT id FROM catalog_option_values WHERE option_id=$1', [material])).rows[0].id;
  await page.goto(origin + `/admin/catalog/options/${color}?page=11`);
  assert.equal(await page.locator('.option-value-row').count(), 1);
  assert.equal(await page.getByLabel('Valore Colore 501', { exact: true }).inputValue(), 'Colore 501');
  await page.getByLabel('Nuovo valore', { exact: true }).fill('Colore 502');
  await page.getByRole('button', { name: 'Aggiungi valore', exact: true }).click();
  await page.waitForURL(url => url.searchParams.get('saved') === '1' && url.searchParams.get('page') === '11');
  assert.equal(await page.locator('.option-value-row').count(), 2);
  const lastValue = (await query("SELECT id FROM catalog_option_values WHERE option_id=$1 AND name='Colore 502'", [color])).rows[0].id;
  await page.goto(origin + '/admin/catalog/options?page=6');
  assert.ok(await page.getByRole('link', { name: 'ZZZ Colore esteso', exact: true }).isVisible());
  assert.ok(await page.getByRole('link', { name: 'ZZZ Taglia estesa', exact: true }).isVisible());
  const product = (await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,category_id,brand_id,tax_class_id) SELECT 'Prodotto esteso','EXTENDED','prodotto-esteso',100,10,'published',category_id,brand_id,tax_class_id FROM catalog_products WHERE model='MUG' RETURNING id")).rows[0].id;
  await page.goto(origin + `/admin/catalog/products/${product}`);
  if (js) await page.getByRole('tab', { name: 'Opzioni e varianti', exact: true }).click();
  else await page.locator('.product-tab-editor .ui-tabs').getByRole('link', { name: 'Opzioni e varianti', exact: true }).click();
  assert.equal(await page.locator('select[name^="option_"]').count(), 3);
  for (const [i, id] of [[1, color], [2, size], [3, material]]) {
    const select = page.getByLabel(`Opzione ${i}`, { exact: true });
    await select.focus();
    await select.selectOption(String(id));
  }
  await page.getByRole('button', { name: 'Salva opzioni prodotto', exact: true }).click();
  await page.waitForURL(url => url.pathname === `/admin/catalog/products/${product}` && url.searchParams.get('saved') === '1' && url.searchParams.get('tab') === 'variants');
  await query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT $1,ARRAY[v.id,$3::bigint,$4::bigint],'EXTENDED-'||lpad(row_number() OVER(ORDER BY v.id)::text,3,'0'),100,10 FROM catalog_option_values v WHERE v.option_id=$2 AND v.name<>'Colore 502' ORDER BY v.id", [product, color, sizeValue, materialValue]);
  await page.goto(origin + `/admin/catalog/products/${product}/variants/new`);
  assert.equal(await page.getByLabel('ZZZ Colore esteso', { exact: true }).locator('option').count(), 503);
  await page.getByLabel('ZZZ Colore esteso', { exact: true }).selectOption(String(lastValue));
  await page.getByLabel('ZZZ Taglia estesa', { exact: true }).selectOption(String(sizeValue));
  await page.getByLabel('ZZZ Materiale esteso', { exact: true }).selectOption(String(materialValue));
  await page.getByLabel('SKU variante', { exact: true }).fill('EXTENDED-502');
  await page.getByLabel('Prezzo variante (€)', { exact: true }).fill('2,00');
  await page.getByLabel('Quantità variante', { exact: true }).fill('10');
  await page.getByLabel('Variante attiva', { exact: true }).check();
  await page.getByRole('button', { name: 'Crea variante', exact: true }).click();
  await page.waitForURL(url => url.searchParams.get('saved') === '1');
  assert.equal(Number((await query('SELECT count(*) FROM catalog_variants WHERE product_id=$1', [product])).rows[0].count), 502);
  await page.goto(origin + `/admin/catalog/products/${product}/variants?page=26`);
  assert.equal(await page.locator('tbody tr').count(), 2);
  assert.ok(await page.getByText('EXTENDED-502', { exact: true }).isVisible());
  await page.goto(origin + '/products/prodotto-esteso');
  for (const [label, id] of [['ZZZ Colore esteso', lastValue], ['ZZZ Taglia estesa', sizeValue], ['ZZZ Materiale esteso', materialValue]]) {
    const select = page.getByLabel(label, { exact: true });
    await select.focus();
    if (js) await page.evaluate(() => { window.extendedSettled = false; document.body.addEventListener('htmx:afterSettle', () => { window.extendedSettled = true; }, { once: true }); });
    await select.selectOption(String(id));
    if (js) {
      await page.waitForFunction(() => window.extendedSettled === true);
      assert.ok(await page.getByLabel(label, { exact: true }).evaluate(el => el === document.activeElement));
    }
  }
  if (!js) {
    await page.getByRole('button', { name: 'Mostra variante', exact: true }).click();
    await page.waitForURL(url => url.searchParams.get('value_3') === String(materialValue));
  }
  assert.equal(await page.locator('[data-product-sku]').innerText(), 'EXTENDED-502');
  assert.equal(await page.locator('[data-product-price]').innerText(), '2,00 € IVA inclusa');
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({ path: path.join(output, `unlimited-options-${name}.png`), fullPage: true });
};
