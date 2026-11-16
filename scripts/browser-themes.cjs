const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require('playwright');
const presentation=require('./theme-presentation-helper.cjs');
(async()=>{
 const {origin,admin}=JSON.parse(fs.readFileSync(0,'utf8'));
 const out=path.resolve('.local/verification/theme-boundary/browser');fs.mkdirSync(out,{recursive:true});
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{}),results=[];
 try {
  for(const[profile,width,js]of[['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]) for(const locale of ['en','it']) {
   const owner=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await owner.addCookies([{name:'talocart_session',value:admin,url:origin}]);
   const buyer=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
   try {
    const settings=await owner.newPage(),page=await buyer.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await settings.goto(origin+'/admin/account');await settings.locator('#account-language').selectOption(locale);await settings.locator('form[action="/admin/account/language"] button').click();await settings.waitForLoadState('load');
    const select=async theme=>{
     await settings.goto(origin+'/admin/settings/storefront');
     await settings.locator('#theme-id').selectOption(theme);
     await settings.locator('#settings-save').focus();await settings.keyboard.press('Enter');
     await settings.waitForURL('**/admin/settings/storefront?saved=1');
     assert.equal(await settings.locator('#theme-id').inputValue(),theme);
     assert.equal(await settings.locator('link[href="/assets/themes/essential.css"]').count(),0);
     { const shot = {path:path.join(out,`${profile}-${locale}-${theme}-settings.png`),fullPage:true}; await presentation.verify(settings, path.basename(shot.path)); await presentation.screenshot(settings, shot); }
    };
    const url=p=>origin+(locale==='it'?'/it':'')+p;
    let expectedFooter;
    const checkFooter=async target=>{
     await target.waitForLoadState('load');
     const footer=target.locator('.site-footer');
     assert.equal(await footer.count(),1,'One shared footer on '+target.url());
     const markup=await footer.evaluate(e=>{const copy=e.cloneNode(true);copy.querySelectorAll('[aria-current]').forEach(a=>a.removeAttribute('aria-current'));return copy.innerHTML;});
     if(expectedFooter===undefined)expectedFooter=markup;
     assert.equal(markup,expectedFooter,'Identical footer content across public pages and themes: '+target.url());
     assert.ok(await target.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'No overflow: '+target.url());
    };
    // Actual two-tab stale-fragment navigation must produce a full document.
    if(js){
     await select('default');await page.goto(url('/catalog'));await select('essential');
     await page.locator('[data-cart-link]').click();
     await page.waitForURL(url('/cart'));
     assert.equal(await page.locator('body').getAttribute('data-theme'),'essential');
     assert.equal(await page.locator('.site-header').count(),1);
    }
    for(const theme of ['default','essential']) {
     await select(theme);
     for(const route of ['/','/catalog','/catalog/theme-category','/catalog/theme-child','/brands','/brands/theme-brand','/search/method','/products/method-item','/account/login','/account/register','/account/reset','/cart','/pages/theme-info','/theme-missing']) {
      const target=route==='/'&&locale==='it'?origin+'/it':url(route);
      const response=await page.goto(target);assert.equal(response.status(),route==='/theme-missing'?404:200,route);
      assert.equal(await page.locator('body').getAttribute('data-theme'),theme);
      await presentation.verify(page,`${profile}-${locale}-${theme}-${route}`);
      if(route==='/'){
       await page.locator('.category-navigation details').evaluateAll(items=>items.forEach(e=>e.open=true));
       await presentation.verify(page,`${profile}-${locale}-${theme}-open-menu`);
       await page.locator('.category-navigation details').evaluateAll(items=>items.forEach(e=>e.open=false));
      }

      await checkFooter(page);
      const footer=page.locator('.site-footer');
      assert.equal(await footer.count(),1,'One shared document footer');
      assert.equal(await footer.getAttribute('data-store-name'),'Shop');
      assert.deepEqual(await footer.locator('h2').allTextContents(),locale==='it'?['Esplora','I tuoi acquisti','Informazioni']:['Explore','Your purchases','Information']);
      for(const path of ['/catalog','/brands','/account','/cart','/pages/theme-info']) assert.equal(await footer.locator('a[href="'+new URL(url(path)).pathname+'"]').count(),1,'Localized footer '+path);
      if(route==='/pages/theme-info') assert.equal(await footer.locator('a[aria-current="page"]').count(),1);
      if(route==='/'){
       const catalog=footer.locator('a[href$="/catalog"]'),brands=footer.locator('a[href$="/brands"]');
       await catalog.focus();await page.keyboard.press('Tab');assert.ok(await brands.evaluate(e=>e===document.activeElement&&getComputedStyle(e).outlineStyle!=='none'),'Visible keyboard focus');
       await page.keyboard.press('Enter');await page.waitForURL(url('/brands'));await page.goto(target);
       await page.setViewportSize({width:320,height:1000});
       assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Footer at 320px');
       for(const link of await page.locator('.site-footer .store-footer-nav a').all()) assert.ok((await link.boundingBox()).height>=44,'Touch target');
       await page.locator('.site-footer').evaluate(footer=>{
        footer.querySelector('.wordmark span').textContent='A'.repeat(120);
        footer.querySelector('small').textContent='© '+'A'.repeat(120);
        footer.querySelectorAll('nav a').forEach(a=>a.textContent='B'.repeat(120));
        const logo=document.createElement('img');logo.className='brand-logo';logo.width=2000;logo.height=1000;logo.alt='';
        logo.src='data:image/svg+xml,%3Csvg xmlns="http://www.w3.org/2000/svg" width="2000" height="1000"%3E%3C/svg%3E';
        footer.querySelector('.wordmark').prepend(logo);
       });
       assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Long footer labels and large logo at 320px');
       await page.goto(target);
       await page.setViewportSize({width,height:1000});
       await page.locator('.site-footer').screenshot({path:path.join(out,`${profile}-${locale}-${theme}-footer.png`)});
      }
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),route);
      if(['/','/catalog','/products/method-item','/account/login','/pages/theme-info','/theme-missing'].includes(route)){ const shot = {path:path.join(out,`${profile}-${locale}-${theme}-${route.replaceAll('/','_')||'home'}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     }
     await page.goto(url('/products/method-item'));await page.locator('.store-cart-add button').click();await page.waitForURL(u=>u.pathname.endsWith('/cart'));
     { const shot = {path:path.join(out,`${profile}-${locale}-${theme}-cart.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     await page.goto(url('/checkout'));assert.equal(await page.locator('#checkout-details').count(),1);
     { const shot = {path:path.join(out,`${profile}-${locale}-${theme}-checkout.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
     await checkFooter(page);
     if(js){await page.locator('[data-cart-link]').click();await page.waitForFunction(()=>document.querySelector('#minicart')?.open&&document.querySelector('[data-minicart-count]'));{ const shot = {path:path.join(out,`${profile}-${locale}-${theme}-minicart.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }await page.keyboard.press('Escape');}
     // Follow native checkout through validation, saved methods and a real test receipt.
     await page.locator('#email').fill('not-an-email');
     await page.locator('#checkout-details button[value=continue]').click();
     await page.locator('#co-errors').waitFor();await checkFooter(page);
     for(const [id,value] of Object.entries({email:'footer@example.test',first_name:'Alex',last_name:'Example',phone:'+393331234567',shipping_street:'Example',shipping_number:'1',shipping_postal:'00100',shipping_city:'Roma',shipping_region:'RM'})) await page.locator('#'+id).fill(value);
     await page.locator('#shipping_country').selectOption('IT');
     await page.locator('#checkout-details button[value=continue]').click();await page.waitForURL(url('/checkout/methods'));await checkFooter(page);
     await page.locator('[name=shipping_method_id][value="1"]').check();
     await page.locator('[name=payment_method_id][value="2"]').check();
     if(js) await page.waitForFunction(()=>!document.querySelector('#checkout-confirm').disabled);
     else {await page.locator('[data-co-update]').click();await page.waitForLoadState('load');}
     await checkFooter(page);
     await page.locator('#checkout-confirm').click();await page.waitForURL(u=>u.pathname.includes('/checkout/orders/'));await checkFooter(page);
     await page.locator('.site-footer').screenshot({path:path.join(out,`${profile}-${locale}-${theme}-receipt-footer.png`)});
     // A separate synthetic customer keeps anonymous route checks independent.
     const member=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
     try {
      const account=await member.newPage();await account.goto(url('/account/register'));
      for(const [id,value] of Object.entries({first_name:'Alex',last_name:'Example',email:`footer-${profile}-${locale}-${theme}@example.test`,password:'synthetic footer password',password_confirmation:'synthetic footer password'})) await account.locator('#customer-'+id).fill(value);
      await account.locator('#customer-form button[type=submit]:not([value=refresh])').click();await account.waitForURL(url('/account'));
      for(const route of ['/account','/account/addresses','/account/orders','/account/password']) {await account.goto(url(route));await checkFooter(account);}
     } finally {await member.close();}
     assert.deepEqual(errors,[]);
     results.push({profile,locale,theme,passed:true});
    }
   } finally {await buyer.close();await owner.close();}
  }
 }finally{await browser.close();}
 fs.writeFileSync(path.join(out,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
})().catch(e=>{console.error(e);process.exitCode=1});
