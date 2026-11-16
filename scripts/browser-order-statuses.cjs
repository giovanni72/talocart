const assert = require('node:assert/strict');
const path = require('node:path');
async function checkReturnLink(page, target) {
  const editor=page.url();
  const link=page.locator(`main a[href='${target}']`).first();
  assert.ok(await link.evaluate(e=>e.classList.contains('back-link')&&e.closest('.ui-page-header')!==null),'Editor return link must use the shared page header back link');
  const style=await link.evaluate(e=>{const s=getComputedStyle(e);return {decoration:s.textDecorationLine,display:s.display,size:s.fontSize};});
  assert.equal(style.decoration,'underline');assert.equal(style.display,'inline-block');assert.equal(style.size,'12.8px');
  await link.focus();await page.keyboard.press('Enter');await page.waitForURL('**'+target);
  await page.goto(editor);
}
module.exports = async function orderStatuses({page,db,origin,output,name}) {
  const base='/admin/settings/order-statuses';
  const post=async(button,status=303)=>{
    const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().includes(base));
    await button.click();assert.equal((await response).status(),status);
    if(status===303)await page.waitForURL('**'+base);
  };
  for(const lang of ['it','en']) {
    await db.query('UPDATE administrators SET locale=$1',[lang]);await page.goto(origin+base);
    assert.equal(await page.locator('h1').innerText(),lang==='it'?'Stati d’ordine':'Order statuses');
    assert.equal(await page.locator('[role=listitem]').count(),7);
    assert.equal(await page.locator('.sidebar-submenu [aria-current=page]').count(),1);
    assert.equal(await page.locator('.ui-status-badge__color rect').first().evaluate(e=>getComputedStyle(e).fill),'rgb(183, 121, 31)');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    await page.screenshot({path:path.join(output,`${name}-${lang}-statuses.png`),fullPage:true});
    await page.goto(origin+base+'/new');await checkReturnLink(page,base);
    await page.locator('#status-name_it').fill('Pronto al ritiro');await page.locator('#status-name_en').fill('Ready for collection');await page.locator('#status-color').fill('#e11d48');await page.locator('#status-sort_order').fill('-1');
    const submit=()=>page.locator('main form').first().getByRole('button');
    assert.equal(await submit().innerText(),lang==='it'?'Salva stato':'Save status');
    await post(submit(),422);assert.equal(await page.locator('#status-sort_order').getAttribute('aria-invalid'),'true');assert.equal(await page.locator('#status-color').inputValue(),'#e11d48');
    await page.locator('#status-sort_order').fill('15');await page.locator('#status-name_it').focus();await page.keyboard.press('Tab');assert.equal(await page.locator('#status-name_en').evaluate(e=>e===document.activeElement),true);assert.ok(await page.locator('#status-name_en').evaluate(e=>getComputedStyle(e).outlineStyle!=='none'));
    await post(submit());
    const id=(await db.query("SELECT id FROM order_statuses WHERE name_en='Ready for collection'")).rows[0].id;
    const item=page.locator('[role=listitem]').filter({has:page.locator(`a[href='${base}/${id}']`)});
    assert.equal(await item.locator('.ui-status-badge').innerText(),lang==='it'?'Pronto al ritiro':'Ready for collection');assert.equal(await item.locator('.ui-status-badge__color rect').evaluate(e=>getComputedStyle(e).fill),'rgb(225, 29, 72)');
    await page.goto(origin+base+'/'+id);await checkReturnLink(page,base);assert.equal(await page.locator('#status-color').inputValue(),'#e11d48');
    await page.locator('#status-name_en').fill('<script>Ready</script>');await post(submit());await page.goto(origin+base+'/'+id);assert.equal(await page.locator('#status-name_en').inputValue(),'<script>Ready</script>');assert.equal(await page.locator('main script').count(),0);
    await page.screenshot({path:path.join(output,`${name}-${lang}-status-editor.png`),fullPage:true});
    await db.query('UPDATE payment_configuration SET revision=revision+1');await page.locator('#status-name_it').fill('Valore conservato');await post(submit(),409);assert.equal(await page.locator('#status-name_it').inputValue(),'Valore conservato');
    await page.goto(origin+base+'/'+id);await page.locator('#status-confirm').check();await post(page.locator('main form').last().getByRole('button'));
    assert.equal((await db.query('SELECT count(*) FROM order_statuses WHERE id=$1',[id])).rows[0].count,'0');
    await page.goto(origin+base+'/1');await page.locator('#status-confirm').check();await post(page.locator('main form').last().getByRole('button'),409);assert.equal(await page.locator('#order-status-error').count(),1);
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  }
};

module.exports.checkReturnLink = checkReturnLink;
