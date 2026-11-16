const assert = require('node:assert/strict');
const path = require('node:path');
const {expect} = require('playwright/test');
module.exports = async ({page,context,locale,js,width,origin,output,profile,theme}) => {
  const picker=page.locator('[data-product-columns]'),form=picker.locator('form');
  const input=key=>form.locator('input[name=columns][value="'+key+'"]');
  const open=async()=>{if(!await picker.getAttribute('open').then(value=>value!==null))await picker.locator('summary').click();};
  const save=async action=>{
    await open();
    if(js){const response=page.waitForResponse(r=>r.url().endsWith('/products/columns')&&r.request().method()==='POST');await action();assert.equal((await response).status(),204);await expect(form).not.toHaveAttribute('aria-busy','true');}
    else {await action();await Promise.all([page.waitForNavigation(),form.locator('[data-columns-apply]').click()]);}
  };
  assert.equal(await picker.count(),1,'Column selector present');
  assert.equal(await input('actions').count(),0,'Actions cannot be disabled');
  await open();
  assert.equal(await form.locator('input[name=columns]:checked').count(),13,'All optional columns visible by default');
  assert.equal(await picker.locator('summary').innerText(),locale==='it'?'Colonne':'Columns');
  await picker.locator('summary').focus();await page.keyboard.press('Enter');await expect(picker).not.toHaveAttribute('open');await page.keyboard.press('Enter');
  for(const key of ['price','stock']) {
    const operator=page.locator('#product-'+key+'-op'),value=page.locator('#product-'+key);
    const geometry=await value.evaluate(el=>{
      const op=el.parentElement.querySelector('select'),cell=el.closest('td');
      const rect=node=>{const r=node.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,right:r.right};};
      return {operator:rect(op),value:rect(el),cell:rect(cell)};
    });
    assert.ok(Math.abs(geometry.operator.y-geometry.value.y)<1,key+': operator and value share one row');
    assert.ok(geometry.operator.width>=30&&geometry.operator.width<=36,key+': compact operator');
    assert.ok(geometry.value.width>=56,key+': usable numeric input');
    assert.ok(geometry.value.x-geometry.operator.right>=3&&geometry.value.x-geometry.operator.right<=5,key+': compact gap');
    assert.ok(geometry.value.right<=geometry.cell.right-5,key+': controls stay inside column');
    await operator.focus();await page.keyboard.press('Tab');await expect(value).toBeFocused();
    assert.notEqual(await value.evaluate(el=>getComputedStyle(el).outlineStyle),'none');
  }
  const dateHeader=page.locator('thead th[data-product-column=modified_at]');
  await expect(dateHeader).toBeVisible();
  assert.equal(await dateHeader.locator('.ui-table__sort-label').innerText(),locale==='it'?'Ultima modifica':'Last modified');
  const keys=await page.locator('thead tr').first().locator('th').evaluateAll(items=>items.map(item=>item.dataset.productColumn));
  assert.deepEqual(keys.slice(-2),['modified_at','actions']);
  const dateCell=page.locator('tbody [data-product-column=modified_at]').first();
  assert.equal(await dateCell.innerText(),'15/01/2099\n13:30:00');
  assert.equal(await dateCell.locator('.ui-date-time').count(),1);
  const dateLines=await dateCell.locator('.ui-date-time > span').evaluateAll(items=>items.map(el=>({text:el.textContent,top:el.getBoundingClientRect().top,left:el.getBoundingClientRect().left})));
  assert.equal(dateLines.length,2);assert.ok(dateLines[1].top>dateLines[0].top);assert.ok(Math.abs(dateLines[1].left-dateLines[0].left)<1);
  await picker.locator('summary').click();
  await dateHeader.locator('a').click();
  assert.equal(new URL(page.url()).searchParams.get('sort'),'modified_at');
  assert.equal(await dateHeader.getAttribute('aria-sort'),'ascending');
  await dateHeader.locator('a').click();assert.equal(await dateHeader.getAttribute('aria-sort'),'descending');
  assert.equal(new URL(page.url()).searchParams.get('q'),'Large');
  await page.goto(origin+'/admin/catalog/products?q=Large&type=simple&sort=model&dir=asc&page=2');

  await save(()=>input('modified_at').uncheck());await expect(dateHeader).toBeHidden();
  await page.reload();await expect(dateHeader).toBeHidden();await open();
  await save(()=>input('modified_at').check());await expect(dateHeader).toBeVisible();
  await save(()=>input('model').uncheck());
  await expect(page.locator('thead th[data-product-column=model]')).toBeHidden();
  await expect(page.locator('#product-model')).toBeHidden();
  assert.ok((await page.locator('tbody [data-product-column=model]').evaluateAll(items=>items.every(item=>item.hidden))));
  const current=new URL(page.url());assert.equal(current.searchParams.get('q'),'Large');assert.equal(current.searchParams.get('page'),'2');
  await page.reload();await expect(page.locator('thead th[data-product-column=model]')).toBeHidden();
  const other=await context.browser().newContext({viewport:{width,height:1000},javaScriptEnabled:js});
  try {await other.addCookies(await context.cookies());const tab=await other.newPage();await tab.goto(origin+'/admin/catalog/products');await expect(tab.locator('thead th[data-product-column=model]')).toBeHidden();}finally{await other.close();}
  await page.locator('a[rel=next]').click();await expect(page.locator('thead th[data-product-column=model]')).toBeHidden();
  await open();await save(()=>input('model').check());
  await expect(page.locator('thead th[data-product-column=model]')).toBeVisible();
  if(js){
    await open();await page.route('**/admin/catalog/products/columns',route=>route.fulfill({status:503,body:'Unavailable'}));
    // A fast 503 can restore the checkbox before Playwright checks uncheck()'s postcondition.
    // The click triggers the request; the assertions below verify the intended rollback.
    await input('price').click();await expect(form.locator('[data-columns-error]')).toBeVisible();
    await expect(input('price')).toBeChecked();await expect(page.locator('thead th[data-product-column=price]')).toBeVisible();
    await page.unroute('**/admin/catalog/products/columns');
    await save(()=>input('price').uncheck());
    await save(()=>form.locator('[data-columns-reset]').click());
    assert.equal(await form.locator('input[name=columns]:checked').count(),13);
    await page.keyboard.press('Escape');await expect(picker).not.toHaveAttribute('open');
    assert.equal(await picker.locator('summary').evaluate(el=>el===document.activeElement),true);
  } else {
    if(await picker.getAttribute('open')!==null)await picker.locator('summary').click();
  }
  const info=page.locator('tbody [data-ui-info]').first(),infoButton=info.locator('summary');
  const panel=page.locator('#'+await infoButton.getAttribute('aria-controls'));
  assert.equal(await infoButton.locator('span').evaluate(el=>getComputedStyle(el).borderTopWidth),'0px','Info icon has no circular border');
  await infoButton.scrollIntoViewIfNeeded();
  await infoButton.focus();await page.keyboard.press('Enter');await expect(panel).toBeVisible();
  assert.equal(await panel.innerText(),'Synthetic <b>description</b> & details.\nSecond line.');
  assert.equal(await panel.locator('b').count(),0,'Description is escaped plain text');
  if(js){
    const box=await panel.boundingBox();assert.ok(box.x>=0&&box.x+box.width<=width&&box.y>=0&&box.y+box.height<=1000);
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-description.png')});
    await page.keyboard.press('ArrowDown');await expect(panel).toBeFocused();
    await page.keyboard.press('Escape');await expect(panel).toBeHidden();await expect(infoButton).toBeFocused();
    await infoButton.click();await expect(panel).toBeVisible();await page.locator('h1').click();await expect(panel).toBeHidden();
  }else {await infoButton.click();await expect(panel).toBeHidden();}
  const emptyInfo=page.locator('tbody tr').filter({has:page.locator('[data-product-column=model]',{hasText:'LARGE-043'})}).locator('[data-ui-info]');
  await emptyInfo.locator('summary').click();
  const emptyPanel=page.locator('#'+await emptyInfo.locator('summary').getAttribute('aria-controls'));
  await expect(emptyPanel).toBeVisible();assert.equal(await emptyPanel.innerText(),locale==='it'?'Nessuna descrizione disponibile.':'No description available.');
  if(js)await page.keyboard.press('Escape');else await emptyInfo.locator('summary').click();
  const link=page.locator('[data-ui-image-preview]').first();
  assert.ok((await link.getAttribute('href')).endsWith('/detail'));
  if(!js){const response=await context.request.get(origin+await link.getAttribute('href'));assert.equal(response.status(),200);assert.ok(response.headers()['content-type'].startsWith('image/'));}
  if(js){
    await link.scrollIntoViewIfNeeded();
    await link.hover();
    const tooltip=page.locator('#ui-image-tooltip');await expect(tooltip).toBeVisible();
    await expect(tooltip.locator('img')).toHaveJSProperty('complete',true);
    assert.ok(await tooltip.locator('img').evaluate(img=>img.naturalWidth>0));
    const box=await tooltip.boundingBox(),thumb=await link.boundingBox();
    assert.ok(box.width>thumb.width*3,'Preview enlarged');assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=width&&box.y+box.height<=1000,'Tooltip within viewport');
    await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-tooltip.png')});
    await tooltip.hover();await page.waitForTimeout(180);await expect(tooltip).toBeVisible();
    await page.keyboard.press('Escape');await expect(tooltip).toBeHidden();
    await link.focus();await expect(tooltip).toBeVisible();
    await page.keyboard.press('Escape');await expect(tooltip).toBeHidden();
    await page.keyboard.press('Tab');await expect(tooltip).toBeHidden();
    await link.click();await expect(tooltip).toBeVisible();
    await page.locator('h1').click();await expect(tooltip).toBeHidden();
    await page.goto(origin+'/admin/catalog/products?q=missing-image-result');await open();
    await save(()=>input('brand').uncheck());
    assert.equal(await page.locator('[data-product-empty]').getAttribute('colspan'),'13');
    await save(()=>form.locator('[data-columns-reset]').click());
    await page.keyboard.press('Escape');
  }
  await page.goto(origin+'/admin/catalog/products?q=Large&type=simple&sort=model&dir=asc&page=2');
  const search=page.locator('.product-table-search');
  await page.locator('#product-price').fill('1.00');await page.locator('#product-price-op').selectOption('eq');
  await page.locator('#product-stock').fill('0');await page.locator('#product-stock-op').selectOption('eq');
  await page.locator('#product-modified-date').fill('2099-01-15');
  await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);
  assert.equal(await page.locator('tbody th[scope=row]').count(),20);
  assert.ok(!new URL(page.url()).searchParams.has('page'),'Filtering returns to page one');
  const checkFilters=async()=>{for(const [key,value] of Object.entries({price:'1.00',price_op:'eq',stock:'0',stock_op:'eq',modified_date:'2099-01-15'}))assert.equal(new URL(page.url()).searchParams.get(key),value);};
  await page.locator('a[rel=next]').click();await checkFilters();
  await dateHeader.locator('a').click();await checkFilters();
  await page.locator('#product-page-size').selectOption('10');
  if(!js)await page.locator('.product-page-size button').click();
  await page.waitForURL(url=>url.searchParams.get('per_page')==='10');await checkFilters();
  await page.locator('#product-price').fill('2');await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);await expect(page.locator('[data-product-empty]')).toBeVisible();
  await page.locator('#product-price').fill('1');await page.locator('#product-stock-op').selectOption('gt');await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);await expect(page.locator('[data-product-empty]')).toBeVisible();
  await page.locator('#product-stock-op').selectOption('eq');await page.locator('#product-modified-date').fill('2099-01-16');await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);await expect(page.locator('[data-product-empty]')).toBeVisible();
  await page.locator('.product-filter-reset').click();assert.equal(await page.locator('#product-price').inputValue(),'');assert.equal(await page.locator('#product-stock').inputValue(),'');assert.equal(await page.locator('#product-modified-date').inputValue(),'');
  const idValue=page.locator('#product-id');
  await expect(idValue).toBeVisible();
  assert.equal(await idValue.getAttribute('aria-label'),null);
  assert.equal(await page.getByLabel(locale==='it'?'Filtra per ID':'Filter by ID',{exact:true}).count(),1);
  const idHeader=page.locator('thead th[data-product-column=id]');
  assert.equal(await idHeader.evaluate(el=>getComputedStyle(el).textAlign),'center');
  const headerCenter=await idHeader.evaluate(el=>{const r=el.getBoundingClientRect(),a=el.querySelector('.ui-table__sort-label').getBoundingClientRect(),b=el.querySelector('.ui-table__sort-indicator').getBoundingClientRect();return {offset:Math.abs((a.left+a.right)/2-(r.left+r.right)/2),indicator:b.left>=a.right&&b.right<=r.right};});
  assert.ok(headerCenter.offset<1&&headerCenter.indicator,'ID label centered like its values, indicator beside it inside the cell');
  const firstId=(await page.locator('tbody td[data-product-column=id]').first().innerText()).trim();
  assert.equal(await page.locator('tbody td[data-product-column=id]').first().evaluate(el=>getComputedStyle(el).textAlign),'center');
  await idValue.focus();await idValue.fill(firstId);await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);
  assert.equal(await page.locator('tbody th[scope=row]').count(),1);assert.equal((await page.locator('tbody td[data-product-column=id]').innerText()).trim(),firstId);
  assert.ok(!new URL(page.url()).searchParams.has('page'));
  await idHeader.locator('a').click();assert.equal(new URL(page.url()).searchParams.get('id'),firstId);
  await page.locator('#product-page-size').selectOption('20');if(!js)await page.locator('.product-page-size button').click();await page.waitForURL(url=>url.searchParams.get('per_page')==='20');assert.equal(new URL(page.url()).searchParams.get('id'),firstId);
  await save(()=>input('id').uncheck());await expect(idValue).toBeHidden();assert.equal(await page.locator('tbody th[scope=row]').count(),1);
  assert.equal(new URL(page.url()).searchParams.get('id'),firstId);await save(()=>input('id').check());
  if(await picker.getAttribute('open')!==null)await picker.locator('summary').click();
  await page.locator('#product-stock').fill('0');await page.locator('#product-stock-op').selectOption('gt');await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);await expect(page.locator('[data-product-empty]')).toBeVisible();
  await page.locator('#product-stock').fill('');await idValue.fill('9223372036854775807');await Promise.all([page.waitForNavigation(),search.locator('button[type=submit]').click()]);await expect(page.locator('[data-product-empty]')).toBeVisible();
  await page.locator('.product-filter-reset').click();assert.equal(await idValue.inputValue(),'');
  await page.goto(origin+'/admin/catalog/products?q=missing-action-test');await open();
  await form.locator('input[name=columns]').evaluateAll(items=>items.forEach(el=>{el.checked=false;}));
  if(js){const response=page.waitForResponse(r=>r.url().endsWith('/products/columns'));await form.evaluate(el=>el.requestSubmit());assert.equal((await response).status(),204);await expect(form).not.toHaveAttribute('aria-busy','true');}
  else await Promise.all([page.waitForNavigation(),form.locator('[data-columns-apply]').click()]);
  await page.reload();
  await expect(page.locator('thead th[data-product-column=actions]')).toBeVisible();
  await expect(page.locator('.product-table-search button[type=submit]')).toBeVisible();
  await expect(page.locator('.product-filter-reset')).toBeVisible();
  assert.equal(await page.locator('[data-product-empty]').getAttribute('colspan'),'1');
  await page.locator('.product-filter-reset').click();
  await expect(page.locator('tbody [data-product-column=actions]').first()).toBeVisible();
  if(js)await save(()=>form.locator('[data-columns-reset]').click());
  else await save(()=>form.locator('input[name=columns]').evaluateAll(items=>items.forEach(el=>{el.checked=true;})));
  await page.goto(origin+'/admin/catalog/products');await open();
  await page.screenshot({path:path.join(output,theme+'-'+locale+'-'+profile+'-columns.png')});
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
};
