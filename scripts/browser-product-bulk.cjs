// Synthetic rows only; called inside the existing isolated demo schema in CI.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, query, origin, js, output, name}) => {
  const {rows: fixtures} = await query("INSERT INTO catalog_products(name,model,slug,price_cents) SELECT 'Azione multipla '||i,'BULK-TEST-'||i,'bulk-test-'||i,100 FROM generate_series(1,3) i RETURNING id");
  const target = origin + '/admin/catalog/products?model=BULK-TEST&sort=id&dir=desc&per_page=10';
  await page.goto(target);
  const button = page.getByRole('button',{name:'Azioni multiple',exact:true});
  const selected = page.locator('.product-row-select');
  assert.equal(await selected.count(),3);
  if (js) assert.equal(await button.isDisabled(),true);
  else {
    const empty = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith('/bulk'));
    await button.click();
    assert.equal((await empty).status(),422);
    await page.getByRole('alert').waitFor();
    await page.getByRole('link',{name:'Torna ai prodotti',exact:true}).click();
  }
  await selected.first().focus(); await page.keyboard.press('Space');
  await selected.nth(1).check();
  assert.equal(await button.isEnabled(),true);
  const ids = await page.locator('.product-row-select:checked').evaluateAll(rows => rows.map(row => Number(row.value)));
  const preview = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith('/bulk'));
  await button.click(); assert.equal((await preview).status(),200);
  await page.getByRole('heading',{name:'Azioni multiple',exact:true}).waitFor();
  assert.equal(await page.locator('.product-table').count(),0);
  await page.getByText('2 prodotti selezionati.',{exact:true}).waitFor();
  assert.equal(await page.getByText('Azione multipla 2',{exact:true}).count(),0);
  assert.equal(await page.getByRole('checkbox',{name:'Elimina',exact:true}).isChecked(),false);
  assert.deepEqual((await query("SELECT id FROM catalog_products WHERE model LIKE 'BULK-TEST-%' ORDER BY id")).rows.map(r=>Number(r.id)),fixtures.map(r=>Number(r.id)));
  assert.equal(await page.getByRole('checkbox',{name:'Confermo le azioni su tutti i 2 prodotti selezionati',exact:true}).isChecked(),false);
  await page.screenshot({path:path.join(output,`product-bulk-${name}.png`),fullPage:true});
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),'Bulk page fits the viewport');
  await page.getByRole('link',{name:'Annulla',exact:true}).click();
  assert.equal(new URL(page.url()).searchParams.get('model'),'BULK-TEST');
  assert.equal(new URL(page.url()).searchParams.get('sort'),'id');
  assert.equal(new URL(page.url()).searchParams.get('dir'),'desc');
  assert.equal(new URL(page.url()).searchParams.get('per_page'),'10');
  assert.equal(await page.locator('.product-row-select:checked').count(),0);
  if (js) {
    await page.getByRole('checkbox',{name:'Seleziona tutti gli articoli visibili',exact:true}).check();
    assert.equal(await page.locator('.product-row-select:checked').count(),3);
    await selected.last().uncheck();
  } else { await selected.first().check(); await selected.nth(1).check(); }
  assert.deepEqual(await page.locator('.product-row-select:checked').evaluateAll(rows => rows.map(row=>Number(row.value))),ids);
  await button.click();
  await page.getByRole('checkbox',{name:'Elimina',exact:true}).check();
  const confirmation = page.getByRole('checkbox',{name:'Confermo le azioni su tutti i 2 prodotti selezionati',exact:true});
  await confirmation.focus(); await page.keyboard.press('Space');
  await page.getByRole('button',{name:'Applica azioni',exact:true}).click();
  await page.waitForURL(url => url.searchParams.get('bulk_deleted') === '1');
  await page.getByRole('status').filter({hasText:'Prodotti selezionati eliminati.'}).waitFor();
  assert.equal(await selected.count(),1);
  assert.equal(await page.locator('.product-row-select:checked').count(),0);
  const remaining = (await query("SELECT id FROM catalog_products WHERE model LIKE 'BULK-TEST-%'")).rows;
  assert.equal(remaining.length,1); assert.ok(!ids.includes(Number(remaining[0].id)));
  // Complete the synthetic products with active references, then exercise both state actions.
  await query("INSERT INTO catalog_products(name,model,slug,price_cents) SELECT 'Azione multipla '||i,'BULK-TEST-'||i,'bulk-test-'||i,100 FROM generate_series(4,5) i");
  await query("UPDATE catalog_products SET category_id=(SELECT id FROM catalog_categories WHERE active ORDER BY id LIMIT 1),tax_class_id=(SELECT t.id FROM tax_classes t JOIN tax_rates r ON r.id=t.rate_id WHERE t.active AND r.active ORDER BY t.id LIMIT 1) WHERE model LIKE 'BULK-TEST-%'");
  const beginStateAction = async action => {
    await page.goto(target);
    await selected.first().check(); await selected.nth(1).check();
    const ids = await page.locator('.product-row-select:checked').evaluateAll(rows=>rows.map(row=>Number(row.value)));
    await button.click();
    assert.equal(await page.locator('.product-table').count(),0);
    await page.getByRole('checkbox',{name:action==='publish'?'Pubblica prodotto':'Bozza prodotto',exact:true}).check();
    const confirm = page.getByRole('checkbox',{name:'Confermo le azioni su tutti i 2 prodotti selezionati',exact:true});
    await confirm.focus(); await page.keyboard.press('Space');
    return ids;
  };
  let stateIds = [];
  for (const [action,status,message] of [['publish','published','Prodotti selezionati pubblicati.'],['draft','draft','Prodotti selezionati impostati come bozza.']]) {
    const chosen = await beginStateAction(action);
    if (stateIds.length) assert.deepEqual(chosen,stateIds); else stateIds=chosen;
    await page.getByRole('button',{name:'Applica azioni',exact:true}).click();
    await page.waitForURL(url => url.searchParams.get('bulk_status') === status);
    await page.getByRole('status').filter({hasText:message}).waitFor();
    assert.equal(new URL(page.url()).searchParams.get('model'),'BULK-TEST');
    assert.equal(await page.locator('.product-row-select:checked').count(),0);
    const all = (await query("SELECT id,slug,status FROM catalog_products WHERE model LIKE 'BULK-TEST-%'")).rows;
    for (const row of all) {
      assert.equal(row.status,chosen.includes(Number(row.id))?status:'draft');
      assert.equal((await page.request.get(origin+'/products/'+row.slug)).status(),chosen.includes(Number(row.id)) && status === 'published' ? 200 : 404);
    }
  }
  // An incomplete selected product rejects the entire publication batch and retains the action.
  await query("UPDATE catalog_products SET tax_class_id=NULL WHERE id=$1",[stateIds[0]]);
  await beginStateAction('publish');
  const rejected = page.waitForResponse(r=>r.request().method()==='POST' && r.url().endsWith('/bulk/apply'));
  await page.getByRole('button',{name:'Applica azioni',exact:true}).click();
  assert.equal((await rejected).status(),409);
  await page.getByRole('alert').waitFor();
  assert.equal(await page.getByRole('checkbox',{name:'Pubblica prodotto',exact:true}).isChecked(),true);
  assert.equal(await page.getByRole('checkbox',{name:'Confermo le azioni su tutti i 2 prodotti selezionati',exact:true}).isChecked(),false);
  assert.ok((await query("SELECT status FROM catalog_products WHERE id=ANY($1::bigint[])",[stateIds])).rows.every(row=>row.status==='draft'));
  await page.screenshot({path:path.join(output,`product-bulk-publication-error-${name}.png`),fullPage:true});
  await query("DELETE FROM catalog_products WHERE model LIKE 'BULK-TEST-%'");
};
