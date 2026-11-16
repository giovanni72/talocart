// Runs inside the demo suite's disposable database, with and without JavaScript.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, origin, output, name, query}) => {
  const columns = [['id','ID'],['image','Immagine'],['type','Tipo'],['name','Nome'],['model','Modello'],['category','Categoria'],['brand','Marchio'],['price','Prezzo'],['variants','Varianti'],['stock','Scorte'],['status','Stato']];
  const source = (await query('SELECT p.id,p.image_key,p.price_cents,p.stock FROM catalog_products p')).rows;
  const configured = new Set((await query('SELECT DISTINCT product_id FROM catalog_product_options')).rows.map(row => row.product_id));
  const variants = (await query('SELECT product_id,price_cents,stock,active FROM catalog_variants')).rows;
  const values = new Map(source.map(row => {
    const all = variants.filter(v => v.product_id === row.id), active = all.filter(v => v.active);
    const hasVariants = configured.has(row.id);
    return [row.id, {id:Number(row.id),image:Number(Boolean(row.image_key)),type:Number(hasVariants),
      price:hasVariants ? (active.length ? Math.min(...active.map(v => Number(v.price_cents))) : null) : Number(row.price_cents),
      stock:hasVariants ? (active.length ? active.reduce((sum,v) => sum + v.stock, 0) : null) : row.stock,
      variants:hasVariants ? all.length : null}];
  }));
  for (const [key,label] of columns) {
    await page.goto(origin + '/admin/catalog/products');
    const firstDirection = key === 'name' ? 'desc' : 'asc';
    for (const dir of [firstDirection, firstDirection === 'asc' ? 'desc' : 'asc']) {
      const header = page.locator('thead th').filter({has:page.getByText(label,{exact:true})});
      await header.getByRole('link', {name:`Ordina per ${label} in ordine ${dir === 'asc' ? 'crescente' : 'decrescente'}`,exact:true}).click();
      assert.equal(new URL(page.url()).searchParams.get('sort'),key);
      assert.equal(new URL(page.url()).searchParams.get('dir'),dir);
      assert.equal(await header.getAttribute('aria-sort'),dir === 'asc' ? 'ascending' : 'descending');
      const rowIds = await page.locator('.ui-table__link').evaluateAll(links => links.map(link => Number(new URL(link.href).pathname.split('/').pop())));
      if (['id','image','type','price','stock','variants'].includes(key)) {
        const sorted = rowIds.map(id => values.get(String(id))?.[key] ?? values.get(id)?.[key] ?? null);
        for (let i=1; i<sorted.length; i++) {
          if (sorted[i-1] === null) assert.equal(sorted[i], null, 'Missing values remain last');
          else if (sorted[i] !== null) assert.ok(dir === 'asc' ? sorted[i-1] <= sorted[i] : sorted[i-1] >= sorted[i], `${key} is numerically ${dir}`);
        }
      }
    }
  }
  await page.goto(origin + '/admin/catalog/products');
  await page.getByLabel('Filtra per tipo',{exact:true}).selectOption('simple');
  await page.getByRole('button',{name:'Filtra',exact:true}).click();
  await page.getByRole('link',{name:'Ordina per Modello in ordine crescente',exact:true}).click();
  await page.getByRole('link',{name:'Ordina per Modello in ordine decrescente',exact:true}).focus();
  await page.keyboard.press('Enter');
  assert.equal(await page.getByLabel('Filtra per tipo',{exact:true}).inputValue(),'simple');
  await page.screenshot({path:path.join(output,`product-sorting-${name}.png`),fullPage:false});
  const first = await page.locator('.ui-table__link').allTextContents();
  await page.getByRole('link',{name:'Successivo',exact:true}).click();
  assert.equal(new URL(page.url()).searchParams.get('sort'),'model');
  assert.equal(new URL(page.url()).searchParams.get('dir'),'desc');
  assert.equal(new URL(page.url()).searchParams.get('type'),'simple');
  const second = await page.locator('.ui-table__link').allTextContents();
  assert.ok(second.length > 0 && second.every(label => !first.includes(label)), 'Sorted pages do not overlap');
  await page.getByRole('link',{name:'Ordina per Prezzo in ordine crescente',exact:true}).click();
  assert.equal(new URL(page.url()).searchParams.get('page'),null, 'Changing sort starts on page one');
  await page.getByLabel('Filtra per modello',{exact:true}).fill('BREAKFAST-MUG');
  await page.getByRole('button',{name:'Filtra',exact:true}).click();
  assert.equal(new URL(page.url()).searchParams.get('sort'),'price');
  assert.equal(new URL(page.url()).searchParams.get('dir'),'asc');
  assert.equal(await page.locator('.ui-table__link').count(),1);
  await page.getByLabel('Filtra per modello',{exact:true}).fill('missing-product');
  await page.getByRole('button',{name:'Filtra',exact:true}).click();
  await page.getByRole('heading',{name:'Nessun risultato.',exact:true}).waitFor();
  await page.getByRole('link',{name:'Ordina per Prezzo in ordine decrescente',exact:true}).click();
  assert.equal(await page.locator('th[aria-sort=descending]').count(),1);
  await page.getByRole('link',{name:'Azzera filtri',exact:true}).click();
  assert.equal(new URL(page.url()).search,'');
};
