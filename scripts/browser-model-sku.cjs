// Included in the demo suite: isolated DB, EN/IT, desktop/mobile and native no-JS forms.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, origin, query, output, name}) => {
  const product = (await query("SELECT id,model,slug FROM catalog_products WHERE model='SHIRT'")).rows[0];
  const variant = (await query('SELECT id,sku FROM catalog_variants WHERE product_id=$1 ORDER BY id LIMIT 1',[product.id])).rows[0];
  const otherSku = (await query('SELECT sku FROM catalog_variants WHERE product_id=$1 AND id<>$2 ORDER BY id LIMIT 1',[product.id,variant.id])).rows[0].sku;
  const simple = (await query("SELECT id,model FROM catalog_products WHERE model='MUG'")).rows[0];
  const originalLocale = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  try {
    for (const locale of ['it','en']) {
      await query('UPDATE administrators SET locale=$1',[locale]);
      await page.goto(origin+`/admin/catalog/products/${product.id}/variants/${variant.id}`);
      await page.locator('#variant-sku').fill(product.model);
      const saved = page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.includes('/variants/'));
      await page.getByRole('button',{name:locale==='it'?'Salva variante':'Save variant',exact:true}).click();
      assert.equal((await saved).status(),303);
      assert.equal((await query('SELECT sku FROM catalog_variants WHERE id=$1',[variant.id])).rows[0].sku,product.model);
      await page.goto(origin+`/admin/catalog/products/${simple.id}`);
      const input=page.getByLabel(locale==='it'?'Modello':'Model',{exact:true});
      assert.equal(await input.getAttribute('name'),'model');
      await input.focus();assert.equal(await input.evaluate(e=>e===document.activeElement),true);
      await input.fill(otherSku);
      const productSaved=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname===`/admin/catalog/products/${simple.id}`);
      await page.getByRole('button',{name:locale==='it'?'Salva prodotto':'Save product',exact:true}).click();
      assert.equal((await productSaved).status(),303);
      assert.equal((await query('SELECT model FROM catalog_products WHERE id=$1',[simple.id])).rows[0].model,otherSku);
      await page.goto(origin+'/admin/catalog/products?model='+encodeURIComponent(otherSku)+'&sort=model&dir=asc');
      assert.equal(await page.locator('.ui-table__link').count(),1);
      assert.equal(await page.locator('[name=model][type=search]').inputValue(),otherSku);
      const columns=await page.locator('thead .ui-table__sort-label').allTextContents();
      assert.ok(columns.some(t=>t.trim()===(locale==='it'?'Modello':'Model')));
      await page.screenshot({path:path.join(output,`model-sku-${locale}-${name}.png`)});
      const selections=(await query('SELECT a.position,v.value_id FROM catalog_variant_values v JOIN catalog_option_values x ON x.id=v.value_id JOIN catalog_product_options a ON a.product_id=$1 AND a.option_id=x.option_id WHERE v.variant_id=$2 ORDER BY a.position',[product.id,variant.id])).rows;
      const params=new URLSearchParams(selections.map(v=>['value_'+v.position,String(v.value_id)]));
      await page.goto(origin+(locale==='en'?'/en':'')+'/products/'+product.slug+'?'+params);
      assert.equal(await page.locator('[data-product-model]').innerText(),product.model);
      assert.equal(await page.locator('[data-product-sku]').innerText(),product.model);
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await page.screenshot({path:path.join(output,`model-sku-detail-${locale}-${name}.png`)});
      await query('UPDATE catalog_products SET model=$1 WHERE id=$2',[simple.model,simple.id]);
      await query('UPDATE catalog_variants SET sku=$1 WHERE id=$2',[variant.sku,variant.id]);
    }
  } finally {
    await query('UPDATE catalog_products SET model=$1 WHERE id=$2',[simple.model,simple.id]);
    await query('UPDATE catalog_variants SET sku=$1 WHERE id=$2',[variant.sku,variant.id]);
    await query('UPDATE administrators SET locale=$1',[originalLocale]);
  }
};
