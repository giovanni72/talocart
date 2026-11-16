const assert=require('node:assert/strict');
const path=require('node:path');
module.exports=async({page,query,origin,output,name,js})=>{
 const selected=async()=> (await query('SELECT country_code FROM checkout_countries ORDER BY country_code')).rows.map(r=>r.country_code);
 assert.deepEqual(await selected(),[],'Purchase destinations start empty');
 const fiscal=(await query('SELECT row_to_json(p)::text AS data FROM pricing_settings p')).rows[0].data;
 for(const language of ['it','en']) {
  await page.goto(origin+'/admin/account');const langForm=page.locator('form:has(select[name="locale"])');await langForm.locator('select').selectOption(language);await langForm.locator('button[type=submit]').click();await page.waitForURL('**/admin/account?saved=1');
  await page.goto(origin+'/admin/settings');await page.locator('#settings-tab-countries').click();assert.ok(await page.locator('#countries').isVisible());
  assert.equal(await page.locator('#checkout-countries option').count(),249);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  const form=page.locator('#settings-editor');await page.locator('#checkout-countries').selectOption(['IT','FR']);await page.locator('#settings-save').click();await page.waitForURL(url=>url.searchParams.get('saved')==='1'&&url.pathname===(js?'/admin/settings/countries':'/admin/settings'));assert.deepEqual(await selected(),['FR','IT']);
  await page.locator('#settings-save').focus();assert.equal(await page.locator('#settings-save').evaluate(e=>e===document.activeElement),true);
  await page.screenshot({path:path.join(output,`purchase-countries-${name}-${language}.png`),fullPage:true});
  const other=await page.context().newPage();await other.goto(origin+'/admin/settings/countries');
  await other.locator('#checkout-countries').selectOption(['IT','DE']);await other.locator('#settings-save').click();await other.waitForURL('**/admin/settings/countries?saved=1');await other.close();
  await page.locator('#checkout-countries').selectOption(['IT','US']);const stale=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().endsWith('/settings/save'));await page.locator('#settings-save').click();assert.equal((await stale).status(),409);await page.locator('#settings-error').waitFor();assert.deepEqual(await selected(),['DE','IT']);assert.deepEqual(await page.locator('#checkout-countries').evaluate(e=>[...e.selectedOptions].map(o=>o.value).sort()),['IT','US']);
  await page.locator('[data-settings-discard]').click();
  await page.goto(origin+'/admin/settings/taxes/coverage');assert.ok((await page.locator('h1').innerText()).includes(language==='it'?'Copertura fiscale':'Tax coverage'));assert.equal(await page.locator('[data-country="IT"]').count(),1);assert.ok(await page.locator('.fiscal-navigation').evaluate(e=>getComputedStyle(e).display==='flex' && parseFloat(getComputedStyle(e).gap)>0),'Coverage navigation uses spaced wrapping actions');assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await page.screenshot({path:path.join(output,`coverage-${name}-${language}.png`),fullPage:true});
  await page.goto(origin+'/admin/settings/countries');await page.locator('#checkout-countries').selectOption(['IT','FR']);await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/countries?saved=1');

  const csrf=await page.locator('#settings-editor input[name=csrf]').inputValue();
  const response=await page.context().request.post(origin+'/admin/settings/countries',{form:{csrf,revision:await form.locator('[name=fiscal_revision]').inputValue(),country:'ZZ'}});assert.equal(response.status(),422);assert.ok((await response.text()).includes('ZZ'));assert.deepEqual(await selected(),['FR','IT']);
  await page.locator('#checkout-countries').selectOption(['IT','US']);await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/countries?saved=1');assert.deepEqual(await selected(),['IT','US']);
  await page.locator('#checkout-countries').selectOption([]);await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/countries?saved=1');assert.deepEqual(await selected(),[]);assert.ok((await page.locator('#countries-empty').innerText()).includes(language==='it'?'acquisti sono bloccati':'purchasing is blocked'));
 }
 // Catalog visibility and taxes are independent from the destination allowlist.
 assert.equal((await query('SELECT row_to_json(p)::text AS data FROM pricing_settings p')).rows[0].data,fiscal);
 await page.goto(origin+'/catalog');assert.ok((await page.locator('main').innerText()).includes('USD'));await page.goto(origin+'/admin/settings/countries');await page.locator('#checkout-countries').selectOption(['IT','DE']);await page.locator('#settings-save').click();await page.waitForURL('**/admin/settings/countries?saved=1');assert.deepEqual(await selected(),['DE','IT']);
};
