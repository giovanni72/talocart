// Runs in the disposable demo schema; temporary variants are removed before other scenarios.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, origin, name, output, query, js}) => {
  const viewport = page.viewportSize();
  const product = (await query("SELECT id FROM catalog_products WHERE model='SHIRT'")).rows[0].id;
  const axes = (await query('SELECT option_id FROM catalog_product_options WHERE product_id=$1 ORDER BY position',[product])).rows;
  const color = (await query('SELECT id FROM catalog_option_values WHERE option_id=$1 ORDER BY id LIMIT 1',[axes[0].option_id])).rows[0].id;
  const route = `/admin/catalog/products/${product}/variants`;
  try {
    for (let index=0; index<23; index++) {
      const value = (await query('INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id',[axes[1].option_id,`Filtro variante ${index}`])).rows[0].id;
      const ids = [color,value].sort((a,b)=>Number(a)-Number(b));
      // The variant_values_sync trigger creates the combination associations.
      await query('INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock,active) VALUES($1,$2,$3,100,0,$4)',[product,ids,`VFILTER-${index}`,index<21]);
    }
    for (const width of [1440,320]) {
      await page.setViewportSize({width,height:1000});
      await page.goto(origin+route);
      const combination = page.getByLabel('Filtra per combinazione',{exact:true});
      const sku = page.getByLabel('Filtra per SKU variante',{exact:true});
      const state = page.getByLabel('Stato variante',{exact:true});
      await combination.fill('filtro variante');
      await combination.focus(); await page.keyboard.press('Tab');
      assert.ok(await sku.evaluate(e=>e===document.activeElement),'Keyboard reaches the SKU filter');
      await sku.fill('vfilter-'); await state.selectOption('active');
      await page.getByRole('button',{name:'Filtra',exact:true}).click();
      assert.equal(await page.locator('.variant-table tbody th[scope=row]').count(),20);
      const pagination = page.getByRole('navigation',{name:'Pagine varianti',exact:true});
      assert.ok((await pagination.innerText()).includes('di 21'));
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No page overflow');
      await require('./ui-contract.cjs')(page);
      await page.screenshot({path:path.join(output,`variant-filters-${name}-${width}.png`)});
      await pagination.getByRole('link',{name:'Successivo',exact:true}).focus(); await page.keyboard.press('Enter');
      await page.waitForURL(url=>url.searchParams.get('page')==='2');
      const params = new URL(page.url()).searchParams;
      assert.equal(params.get('page'),'2'); assert.equal(params.get('combination'),'filtro variante');
      assert.equal(params.get('sku'),'vfilter-'); assert.equal(params.get('status'),'active');
      assert.equal(await page.locator('.variant-table tbody th[scope=row]').count(),1);
      const size = page.getByLabel('Varianti per pagina',{exact:true});
      const apply = page.locator('[data-product-page-size]').getByRole('button',{name:'Applica',exact:true});
      assert.equal(await apply.isVisible(), !js, 'Size selector uses automatic submit with a native no-JS fallback');
      const resized = page.waitForURL(url=>url.searchParams.get('per_page')==='10' && !url.searchParams.has('page'));
      await size.selectOption('10'); if (!js) await apply.click(); await resized;
      assert.equal(await page.locator('.variant-table tbody th[scope=row]').count(),10);
      assert.equal(await page.locator('.catalog-result-count').innerText(),'21 varianti');
      assert.equal(await combination.inputValue(),'filtro variante'); assert.equal(await sku.inputValue(),'vfilter-'); assert.equal(await state.inputValue(),'active');
      for (const quantity of [50,100,20,10]) {
        const navigation = page.waitForURL(url=>url.searchParams.get('per_page')===String(quantity) && !url.searchParams.has('page'));
        await size.selectOption(String(quantity)); if (!js) await apply.click(); await navigation;
        assert.equal(await page.locator('.variant-table tbody th[scope=row]').count(),Math.min(quantity,21));
        assert.equal(await page.locator('.catalog-result-count').innerText(),'21 varianti');
      }
      await pagination.getByRole('link',{name:'Successivo',exact:true}).click();
      assert.equal(new URL(page.url()).searchParams.get('per_page'),'10');
      await sku.fill('absent'); await page.getByRole('button',{name:'Filtra',exact:true}).click();
      assert.equal(new URL(page.url()).searchParams.has('page'),false,'Submitting filters starts at the first page');
      assert.equal(await page.locator('.variant-table .ui-table__filters').count(),1);
      assert.equal(await page.locator('.variant-table tbody td[colspan="7"]').count(),1);
      await page.getByRole('heading',{name:'Nessun risultato.',exact:true}).waitFor();
      assert.equal(await page.locator('.catalog-result-count').innerText(),'0 varianti');
      await page.getByRole('link',{name:'Azzera filtri varianti',exact:true}).click();
      assert.equal(await size.inputValue(),'10'); assert.equal(new URL(page.url()).searchParams.get('per_page'),'10');
      assert.equal(await page.locator('.catalog-result-count').innerText(),'27 varianti');
      assert.equal(await combination.inputValue(),''); assert.equal(await sku.inputValue(),''); assert.equal(await state.inputValue(),'');
      await sku.fill('VFILTER-'); await state.selectOption('inactive');
      await page.getByRole('button',{name:'Filtra',exact:true}).click();
      assert.equal(await page.locator('.variant-table tbody th[scope=row]').count(),2);
      assert.equal(await page.locator('.variant-table tbody .catalog-status.inactive').count(),2);
    }
  } finally {
    await query("DELETE FROM catalog_variants WHERE product_id=$1 AND sku LIKE 'VFILTER-%'",[product]);
    await query("DELETE FROM catalog_option_values WHERE option_id=$1 AND name LIKE 'Filtro variante %'",[axes[1].option_id]);
    await page.setViewportSize(viewport);
  }
};
