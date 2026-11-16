// Native row selection and progressive select-all on the current result page.
const assert = require('node:assert/strict');
module.exports = async ({page, origin, name}) => {
  const js = !['no-js', 'mobile-no-js'].includes(name);
  await page.goto(origin + '/admin/catalog/products');
  const rows = page.locator('.product-row-select');
  const all = page.getByRole('checkbox', {name:'Seleziona tutti gli articoli visibili',exact:true});
  const selected = () => page.locator('.product-row-select:checked').count();
  const visibleIds = await page.locator('.ui-table__link').evaluateAll(links => links.map(link => new URL(link.href).pathname.split('/').pop()));
  assert.deepEqual(await rows.evaluateAll(inputs => inputs.map(input => input.value)),visibleIds);
  assert.deepEqual(await page.locator('.product-id-cell').allTextContents(),visibleIds);
  assert.equal(await page.locator('.product-table thead tr:first-child th').count(),14);
  assert.equal(await page.locator('.ui-table__filters td:first-child #product-select-all').count(),1);
  assert.equal(await rows.count(),20);
  assert.equal(await selected(),0);
  await rows.first().focus();
  await page.keyboard.press('Space');
  assert.equal(await selected(),1,'Space selects a row without navigating');
  assert.equal(await all.isDisabled(),!js);
  if (js) {
    assert.equal(await all.evaluate(e => e.indeterminate),true);
    await all.check();
    assert.equal(await selected(),20,'Select all covers only the visible twenty rows');
    assert.equal(await all.evaluate(e => e.indeterminate),false);
    await rows.first().uncheck();
    assert.equal(await selected(),19);
    assert.equal(await all.isChecked(),false);
    assert.equal(await all.evaluate(e => e.indeterminate),true);
    await all.check();
    await all.focus();
    await page.keyboard.press('Space');
    assert.equal(await selected(),0,'Space on select all clears the visible selection');
  } else {
    await rows.first().uncheck();
    assert.equal(await selected(),0,'Individual selection works without JavaScript');
  }
  await rows.first().check();
  await page.getByRole('link',{name:'Successivo',exact:true}).click();
  await page.waitForURL(url => url.searchParams.get('page') === '2');
  assert.equal(await rows.count(),10);
  assert.equal(await selected(),0,'Selection starts afresh on another page');
  if (js) {
    await all.check();
    assert.equal(await selected(),10,'Select all on page two selects only its ten rows');
  } else await rows.first().check();
  await page.getByLabel('Filtra per modello',{exact:true}).fill('BREAKFAST-MUG');
  await page.getByRole('button',{name:'Filtra',exact:true}).click();
  assert.equal(await rows.count(),1);
  assert.equal(await selected(),0);
  assert.equal(new URL(page.url()).searchParams.get('page'),null);
  assert.ok([...new URL(page.url()).searchParams.keys()].every(key => ['id','q','type','model','category','brand','status','sort','dir','per_page','price','price_op','stock','stock_op','modified_date'].includes(key)), 'Checkbox values never enter the search query');
  if (js) {
    await rows.first().check();
    assert.equal(await all.isChecked(),true,'One filtered row fully selects its page');
    assert.equal(await all.evaluate(e => e.indeterminate),false);
    await all.uncheck();
  } else await rows.first().check();
  await page.getByLabel('Filtra per modello',{exact:true}).fill('no-such-product');
  await page.getByRole('button',{name:'Filtra',exact:true}).click();
  await page.getByRole('heading',{name:'Nessun risultato.',exact:true}).waitFor();
  assert.equal(await rows.count(),0);
  assert.equal(await all.isDisabled(),true,'Empty results cannot be selected');
  assert.equal(await page.locator('.product-table tbody td').getAttribute('colspan'),'14');
  await page.getByRole('link',{name:'Azzera filtri',exact:true}).click();
  assert.equal(await rows.count(),20);
  assert.equal(await selected(),0);
  assert.equal(await all.isDisabled(),!js);
};
