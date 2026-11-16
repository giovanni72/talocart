const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,origin,name,js,output,query}) => {
  const primary=(await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale;
  for (const locale of ['en','it']) {
    await page.goto(origin+'/admin/account');
    const previous = await page.locator('html').getAttribute('lang');
    await page.locator('#account-language').selectOption(locale);
    await page.getByRole('button',{name:previous==='it'?'Salva lingua':'Save language',exact:true}).click();
    await page.waitForURL('**/admin/account?saved=1');
    // Real administrative flash message, using the same component as the examples.
    await page.goto(origin+'/admin/account?saved=1');
    const saved = page.locator('[data-ui-toast]');
    assert.equal(await saved.count(), 1);
    assert.equal(await saved.locator('[role=status]').count(), 1);
    assert.equal(await saved.locator('button').getAttribute('aria-label'), locale==='it'?'Chiudi messaggio':'Dismiss message');
    // Product guidance is persistent page context, even when a save feedback is present.
    const product=(await query('SELECT p.id FROM catalog_products p WHERE EXISTS (SELECT 1 FROM catalog_product_options o WHERE o.product_id=p.id) ORDER BY p.id LIMIT 1')).rows[0];
    assert.ok(product, 'Fixture includes a product with variants');
    await page.goto(origin+'/admin/catalog/products/'+product.id+'?saved=1');
    await page.locator('#tab-commerce').click();
    const guidance=page.locator('[data-variant-commerce-summary]');
    await guidance.waitFor();
    assert.equal(await guidance.getAttribute('data-ui-toast'),null,'Variant price/stock summary must remain persistent page content');
    assert.equal(await guidance.locator('button').count(),0,'Context has no dismissal');
    assert.equal(await page.locator('#product-form-message-1[data-ui-toast]').count(),1,'Save feedback remains a toast');
    if (js) {
      await page.mouse.move(0,page.viewportSize().height-1);
      await page.locator('#product-form-message-1').waitFor({state:'detached',timeout:8000});
      assert.ok(await guidance.isVisible(),'Guidance remains after feedback expires');
    }
    if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await guidance.screenshot({animations:'disabled',path:path.join(output,`context-banner-${name}-${locale}.png`)});
    // A real rejected form keeps its field feedback alongside the global notification.
    await page.goto(origin+'/admin/catalog/categories/new');
    await page.locator('[name=name]').fill('   ');
    await page.locator('[name=slug]').fill('toast-test');
    await page.locator('form[action="/admin/catalog/categories/new"] button[type=submit]').click();
    await page.locator('#catalog-error [role=alert]').waitFor();
    assert.equal(await page.locator('#name-error').count(), 1);
    if (js) {
      await page.locator('#catalog-error button').click();
      assert.equal(await page.locator('#name-error').count(), 1, 'Field feedback survives toast dismissal');
    }
    // The storefront uses exactly the same localized toast, also without scripts.
    await page.goto(origin+(locale===primary?'':'/'+locale)+'/cart?saved=1');
    await page.locator('#cart-saved [role=status]').waitFor();
    assert.equal(await page.locator('#cart-saved button').getAttribute('aria-label'), locale==='it'?'Chiudi messaggio':'Dismiss message');
    await page.goto(origin+'/admin/components');
    const group = page.locator('[data-ui-toasts]');
    const success = page.locator('#toast-success');
    const error = page.locator('#toast-error');
    assert.equal(await success.locator('[role=status]').count(),1);
    assert.equal(await error.locator('[role=alert]').count(),1);
    assert.equal(await success.locator('button').getAttribute('aria-label'),locale==='it'?'Chiudi messaggio':'Dismiss message');
    assert.equal(await success.locator('button').isVisible(),js);
    if (!js) {
      assert.equal(await group.evaluate(e=>getComputedStyle(e).position),'static');
      if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await group.screenshot({animations:'disabled',path:path.join(output,`toasts-${name}-${locale}.png`)});
      continue;
    }
    const close = success.locator('button');
    const assertPlain = async () => {
      const appearance = await close.evaluate(e => {
        const s = getComputedStyle(e), ring = getComputedStyle(e, '::after');
        const box = e.getBoundingClientRect();
        return {border:s.borderWidth, background:s.backgroundColor, shadow:s.boxShadow,
          ring:ring.content, width:box.width, height:box.height, color:s.color,
          parentColor:getComputedStyle(e.parentElement).color};
      });
      assert.equal(appearance.border, '0px', 'Close icon has no border');
      assert.equal(appearance.background, 'rgba(0, 0, 0, 0)', 'Close icon has no background');
      assert.equal(appearance.shadow, 'none');
      assert.equal(appearance.ring, 'none', 'Close icon has no decorative ring');
      assert.equal(appearance.color, appearance.parentColor);
      assert.ok(appearance.width >= 43.99 && appearance.height >= 43.99, `Icon retains an accessible hit area: ${JSON.stringify(appearance)}`);
    };
    await assertPlain();
    await close.hover(); await assertPlain();
    await page.mouse.move(0,page.viewportSize().height-1);
    const box = await page.locator('[data-ui-toast-viewport]').boundingBox();
    assert.ok(Math.abs(box.x+box.width/2-page.viewportSize().width/2)<1,'Toast group centered');
    assert.ok(box.y>=0 && box.y<32 && box.x>=0 && box.x+box.width<=page.viewportSize().width);
    if (!process.env.TALOCART_TEST_NO_SCREENSHOTS) await page.locator('[data-ui-toast-viewport]').screenshot({animations:'disabled',path:path.join(output,`toasts-${name}-${locale}.png`)});
    await success.locator('button').focus();
    assert.equal(await close.evaluate(e=>getComputedStyle(e).outlineStyle),'solid','Keyboard focus remains visible');
    await page.waitForTimeout(6300);
    assert.equal(await success.count(),1,'Focused toast stays visible');
    assert.equal(await page.locator('#toast-info').count(),0,'Information closes automatically');
    assert.equal(await error.count(),1,'Error stays visible');
    await page.keyboard.press('Enter');
    assert.equal(await success.count(),0,'Keyboard dismissal');
    assert.ok(await error.locator('button').evaluate(e=>e===document.activeElement),'Focus moves to remaining toast');
    await page.keyboard.press('Enter');
    assert.ok(await page.locator('main').evaluate(e=>e===document.activeElement),'Last dismissal returns focus to main');
    // Source groups from separate fragments share one non-overlapping viewport.
    // Simulate a fragment arriving through HTMX, with the real rendered markup.
    await page.getByRole('button',{name:locale==='it'?'Mostra di nuovo i toast':'Show toasts again',exact:true}).click();
    await success.waitFor();
    const markup = await success.evaluate(e=>e.outerHTML);
    const errorMarkup = await error.evaluate(e=>e.outerHTML);
    await page.locator('[data-ui-toast]').evaluateAll(nodes=>nodes.forEach(n=>n.remove()));
    await group.evaluate((e,html)=>{ e.innerHTML=html; },markup);
    await success.locator('button').hover();
    await page.waitForTimeout(6300);
    assert.equal(await success.count(),1,'Hover pauses a dynamically inserted toast');
    await page.mouse.move(0,page.viewportSize().height-1);
    await success.waitFor({state:'detached',timeout:7000});
    await page.locator('main').evaluate((main, html) => {
      for (let i=0;i<2;i++) {
        const source = document.createElement('section'); source.id='fragment-'+i;
        source.innerHTML='<div data-ui-toasts>'+html.replace('id="toast-error"','id="fragment-error-'+i+'"')+'</div>';
        main.append(source);
      }
    }, errorMarkup);
    await page.locator('#fragment-error-1 button').waitFor();
    assert.equal(await page.locator('[data-ui-toast-viewport]').count(), 1);
    const a=await page.locator('#fragment-error-0').boundingBox(), b=await page.locator('#fragment-error-1').boundingBox();
    assert.ok(b.y>=a.y+a.height, 'Independent fragment notifications do not overlap');
    await page.locator('#fragment-0').evaluate(e=>e.remove());
    await page.locator('#fragment-error-0').waitFor({state:'detached'});
    assert.equal(await page.locator('#fragment-error-1').count(), 1, 'Removing one source leaves other notifications intact');
    await page.locator('main').evaluate(main=>{
      const input=document.createElement('input');input.id='toast-described-control';input.setAttribute('aria-describedby','fragment-error-1');main.append(input);
    });
    await page.locator('#fragment-error-1 button').click();
    assert.equal(await page.locator('#fragment-error-1').count(),1,'Description ID survives dismissal');
    assert.equal(await page.locator('#fragment-error-1').isVisible(),false);
    assert.equal(await page.locator('#toast-described-control').evaluate(e=>document.getElementById(e.getAttribute('aria-describedby')).textContent.trim().length>0),true);
    await page.locator('#fragment-1').evaluate(e=>e.remove());
    await page.locator('#fragment-error-1').waitFor({state:'detached'});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  }
};
