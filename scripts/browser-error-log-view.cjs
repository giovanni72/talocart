const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,origin,output,theme,locale,profile}) => {
  await page.goto(origin+'/admin');
  const maintenance=page.locator('.admin-sidebar details').filter({has:page.locator('summary', {hasText:locale==='it'?'Manutenzione':'Maintenance'})});
  await maintenance.locator('summary').focus();await maintenance.locator('summary').press('Enter');
  const link=page.locator('.admin-sidebar a[href="/admin/error-logs"]');
  await link.focus(); await link.press('Enter'); await page.waitForURL('**/admin/error-logs');
  assert.equal(await page.locator('h1').innerText(),locale==='it'?'Registro errori':'Error log');
  await page.locator('#log-date').selectOption('2020-01-02');
  await page.locator('#log-date').focus(); await page.keyboard.press('Tab');
  assert.equal(await page.locator('form[action="/admin/error-logs"] button').evaluate(e=>e===document.activeElement),true);
  await page.locator('form[action="/admin/error-logs"] button').click();
  await page.waitForURL('**/admin/error-logs?date=2020-01-02');
  assert.equal(await page.locator('.error-log-entry').count(),100);
  assert.ok((await page.locator('.error-log-entry h2').first().innerText()).startsWith('Synthetic failure 104'));
  const summary=page.locator('.error-log-entry summary').first();
  await summary.focus(); await summary.press('Enter');
  assert.equal(await page.locator('.error-log-entry details').first().getAttribute('open'),'');
  assert.equal(await page.evaluate(()=>window.logInjected),undefined);
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-error-log.png`),fullPage:false});
  await summary.focus(); await summary.press('Tab');
  const detail = page.locator('.error-log-entry pre').first();
  assert.equal(await detail.evaluate(e=>e===document.activeElement),true,'JSON detail is keyboard focusable');
  assert.ok(await detail.evaluate(e=>e.scrollHeight>e.clientHeight),'Long diagnostic uses a bounded scroll area');
  const waitScroll = async predicate => {
    for (let attempt=0;attempt<40;attempt++) {
      if (await detail.evaluate(predicate)) return;
      await new Promise(resolve=>setTimeout(resolve,50));
    }
    assert.fail('Native keyboard scroll did not reach the expected position');
  };
  await detail.press('End');
  await waitScroll(e=>e.scrollTop>0);
  await detail.press('Home');
  await waitScroll(e=>e.scrollTop===0);
  await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-error-log-detail.png`),fullPage:false});
  await page.locator('main a[rel=next]').click();
  await page.waitForURL(/before=/);
  assert.equal(await page.locator('.error-log-entry').count(),5);
  await page.locator('#log-date').selectOption('2020-01-01');
  await page.locator('form[action="/admin/error-logs"] button').click();
  await page.waitForURL('**/admin/error-logs?date=2020-01-01');
  assert.equal(await page.locator('.ui-empty').count(),1);
};
