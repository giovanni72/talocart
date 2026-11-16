const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require('playwright');
(async () => {
  const { origin, admin } = JSON.parse(fs.readFileSync(0, 'utf8'));
  const out = path.resolve('.local/verification/phase5/browser');
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const results = [];
  try {
    for (const [profile, width, js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]) {
      for (const locale of ['en','it']) {
        const context = await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
        await context.addCookies([{name:'talocart_session',value:admin,url:origin}]);
        try {
          const page = await context.newPage();
          const errors=[]; page.on('pageerror',e=>errors.push(e.message));
          const shot = async name => {
            assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'No horizontal overflow');
            await page.screenshot({path:path.join(out,`${profile}-${locale}-${name}.png`),fullPage:true});
          };
          await page.goto(origin+'/admin/account');
          await page.locator('#account-language').selectOption(locale);
          await page.locator('form[action="/admin/account/language"] button').click();
          await page.waitForLoadState('load');
          await page.goto(origin+'/admin');
          assert.equal(await page.locator('html').getAttribute('lang'),locale);
          assert.equal(await page.locator('h1').innerText(),locale==='it'?'Panoramica':'Overview');
          assert.equal(await page.locator('.dashboard-metric').count(),9);
          assert.equal(await page.locator('[data-metric="dash-products"] .ui-stat__value').innerText(),'1');
          await page.locator('[data-metric="dash-products"] a').focus();
          assert.equal(await page.locator('[data-metric="dash-products"] a').evaluate(e=>e===document.activeElement),true);
          await shot('overview');
          await page.locator('[data-metric="dash-products"] a').click();
          await page.waitForURL('**/admin/catalog/products');
          await page.goto(origin+'/admin');
          assert.equal(await page.locator('#check-status, #status-result').count(),0);
          assert.equal(await page.locator('.dashboard-metric').count(),9);
          await page.goto(origin+'/admin/operations');
          assert.equal(await page.locator('h1').innerText(),locale==='it'?'Registro operazioni':'Operation log');
          await page.locator('#operations-area').selectOption('pages');
          await page.locator('#operations-area').focus();
          await page.keyboard.press('Tab');
          assert.equal(await page.locator('.operations-filter button').evaluate(e=>e===document.activeElement),true);
          await page.locator('.operations-filter button').click();
          await page.waitForURL('**/admin/operations?area=pages');
          assert.equal(await page.locator('.operation-row').count(),20);
          assert.ok(!(await page.locator('main').innerText()).includes('PRIVATE BODY'));
          await shot('operations');
          await page.locator('a[rel=next]').click();
          await page.waitForURL('**/admin/operations?area=pages&page=2');
          assert.equal(await page.locator('.operation-row').count(),5);
          assert.equal(await page.locator('#operations-area').inputValue(),'pages');
          await page.locator('a[rel=prev]').click();
          await page.waitForURL('**/admin/operations?area=pages&page=1');
          await page.locator('#operations-area').selectOption('orders');
          await page.locator('.operations-filter button').click();
          await page.waitForURL('**/admin/operations?area=orders');
          assert.equal(await page.locator('.ui-empty').count(),1);
          await shot('empty');
          for (const [area, labels] of [
            ['catalog', locale === 'it' ? ['Traduzioni prodotti','Traduzioni categorie','Traduzioni marchi'] : ['Product translations','Category translations','Brand translations']],
            ['settings', locale === 'it' ? ['Paesi della zona fiscale','Regole della zona fiscale'] : ['Tax zone countries','Tax zone rules']],
          ]) {
            await page.locator('#operations-area').selectOption(area);
            await page.locator('.operations-filter button').click();
            await page.waitForURL(`**/admin/operations?area=${area}`);
            const content = await page.locator('main').innerText();
            for (const label of labels) assert.ok(content.includes(label),label);
            assert.ok(!content.includes('PRIVATE TRANSLATION'));
            await shot(`related-${area}`);
          }
          // The new permission is assignable and persists through the native role form.
          const roleName = `Log ${profile} ${locale}`;
          await page.goto(origin+'/admin/roles/new');
          await page.locator('#role-name').fill(roleName);
          await page.locator('input[name=operations_read]').check();
          await page.locator('form[action="/admin/roles/new"] button[type=submit]').click();
          await page.waitForURL('**/admin/roles?saved=1');
          await page.locator('.role-row').filter({has:page.getByRole('heading',{name:roleName,exact:true})}).locator('a').click();
          assert.equal(await page.locator('input[name=operations_read]').isChecked(),true);
          assert.equal(await page.locator('input[name=overview_read]').isChecked(),false);
          await shot('role');
          await page.locator('form[action$="/delete"] button').click();
          await page.waitForURL('**/admin/roles?deleted=1');
          assert.deepEqual(errors,[]);
          results.push({profile,locale,passed:true});
        } finally { await context.close(); }
      }
    }
  } finally { await browser.close(); }
  fs.writeFileSync(path.join(out,'results.json'),JSON.stringify(results,null,2));
  console.log(JSON.stringify(results));
})().catch(e=>{console.error(e);process.exitCode=1});
