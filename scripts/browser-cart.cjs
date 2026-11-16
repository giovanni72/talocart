const presentation = require('./theme-presentation-helper.cjs');
// Real native-form cart journeys on dedicated schemas, in both storefront languages.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomBytes } = require('node:crypto');
const { spawn, spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { Client } = require('pg');
const { chromium } = require('playwright');
(async () => {
  assert.ok(process.env.TALOCART_TEST_DATABASE_URL, 'Dedicated test database required');
  const db = new Client({ connectionString: process.env.TALOCART_TEST_DATABASE_URL }); await db.connect();
  const browser = await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL ? { channel: process.env.TALOCART_BROWSER_CHANNEL } : {});
  const output = path.resolve('.local/verification/cart'); fs.mkdirSync(output, { recursive: true });
  const results = [];
  try {
    for (const [name,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]].filter(([name])=>!process.env.TALOCART_CART_TEST_PROFILE||name===process.env.TALOCART_CART_TEST_PROFILE)) {
      const schema = `talocart_cart_browser_${process.pid}_${Date.now()}`;
      const folder = fs.mkdtempSync(path.join(os.tmpdir(),'talocart-cart-'));
      const port = Number(process.env.TALOCART_CART_TEST_PORT || 3114), origin = `http://127.0.0.1:${port}`;
      const url = new URL(process.env.TALOCART_TEST_DATABASE_URL); url.searchParams.set('options',`-csearch_path=${schema}`);
      const config = path.join(folder,'talocart.toml');fs.writeFileSync(config,`[server]\nlisten="127.0.0.1:${port}"\n`,{mode:0o600});
      const env = {...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:`127.0.0.1:${port}`,TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
      const binary = path.resolve(process.env.TALOCART_TEST_BINARY || 'target/release/talocart');
      const cli = command => { const r=spawnSync(binary,['--config',config,command],{env,encoding:'utf8'});assert.equal(r.status,0,`CLI ${command} failed (private output omitted)`); };
      const query=async(sql,args)=>{await db.query(`SET search_path TO ${schema}`);return db.query(sql,args);};
      const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});
      let server;
      const start=async()=>{server=spawn(binary,['--config',config,'serve'],{env,stdio:'ignore'});for(let i=0;i<100;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Server readiness');};
      const stop=async()=>{if(server&&server.exitCode===null){const exited=once(server,'exit');server.kill('SIGTERM');await exited;}};
      try {
        await db.query(`CREATE SCHEMA ${schema}`);cli('migrate');cli('migrate');await start();
        const page=await context.newPage(),errors=[],cspErrors=[];page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error'&&m.text().includes('Content Security Policy'))cspErrors.push(m.text());});
        await context.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);
        await page.goto(origin+'/setup');
        await page.getByLabel('Nome del negozio',{exact:true}).fill('Cart test');await page.getByLabel('Nome visualizzato',{exact:true}).fill('Synthetic owner');
        await page.getByLabel('Email amministratore').fill('owner@example.test');await page.getByLabel('Password',{exact:true}).fill(randomBytes(20).toString('hex'));
        await page.getByRole('button',{name:'Crea negozio',exact:true}).click();await page.waitForURL('**/admin/login');
        if (process.env.TALOCART_CART_TEST_THEME) {
          assert.ok(['default','essential'].includes(process.env.TALOCART_CART_TEST_THEME));
          await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[process.env.TALOCART_CART_TEST_THEME]);
        }
        // Controlled synthetic commercial configuration; no application data is used.
        const rate=(await query("INSERT INTO tax_rates(name,basis_points) VALUES('Synthetic 20',2000) RETURNING id")).rows[0].id;
        const tax=(await query("INSERT INTO tax_classes(name,rate_id) VALUES('Synthetic class',$1) RETURNING id",[rate])).rows[0].id;
        await query("UPDATE pricing_settings SET price_mode='exclusive'");
        const simple=(await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,minimum_quantity,packs,status,tax_class_id) VALUES('Cart simple','CART-SIMPLE','cart-simple',100,100,5,3,'published',$1) RETURNING id",[tax])).rows[0].id;
        const variantProduct=(await query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Cart variant','CART-PARENT','cart-variant',100,100,'published',$1) RETURNING id",[tax])).rows[0].id;
        const option=(await query("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id")).rows[0].id;
        const value=(await query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id",[option])).rows[0].id;
        await query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)",[variantProduct,option]);
        await query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'CART-VARIANT',200,20)",[variantProduct,value]);
        const productImageKey='a'.repeat(64)+'.png',variantImageKey='b'.repeat(64)+'.png';
        fs.mkdirSync(path.join(folder,'media','objects'),{recursive:true});
        const seedImage=fs.readFileSync(path.resolve('seeders/catalog-demo/assets/shirt-sage.png'));
        for(const key of [productImageKey,variantImageKey])fs.writeFileSync(path.join(folder,'media','objects',key),seedImage);
        const productImage=(await query("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',$2,512,512,true) RETURNING id",[productImageKey,seedImage.length])).rows[0].id;
        const variantImage=(await query("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',$2,512,512,true) RETURNING id",[variantImageKey,seedImage.length])).rows[0].id;
        await query('UPDATE catalog_products SET image_key=$1 WHERE id=ANY($2::bigint[])',[productImageKey,[simple,variantProduct]]);
        const primary=(await query('SELECT primary_locale FROM storefront_settings')).rows[0].primary_locale;
        const prefix=locale=>locale===primary?'':`/${locale}`;
        const compactButtons=async(selector)=>{
          const buttons=page.locator(selector).and(page.locator(':not([data-ui-toast-close])'));
          assert.ok(await buttons.count(),selector+' has actions');
          for(const button of await buttons.all()) {
            assert.ok(await button.evaluate(e=>e.classList.contains('ui-button--compact')),'Cart action uses shared compact variant');
            const style=await button.evaluate(e=>{const s=getComputedStyle(e);return {minHeight:s.minHeight,padding:s.padding,fontSize:s.fontSize};});
            assert.deepEqual(style,{minHeight:'36px',padding:'4px 8px',fontSize:'12px'});
          }
        };
        const navigate=async(route,locale)=>{await page.goto(origin+prefix(locale)+route);assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),route+' fits viewport');};
        for(const locale of ['en','it'].filter(locale=>!process.env.TALOCART_CART_TEST_LOCALE||locale===process.env.TALOCART_CART_TEST_LOCALE)) {
          const add=locale==='it'?'Aggiungi al carrello':'Add to cart';
          await context.clearCookies(); // A fresh anonymous shopper for each language.
          const sessionsBefore = Number((await query('SELECT count(*) FROM guest_carts')).rows[0].count);
          const missing = await context.request.get(origin+prefix(locale)+'/products/missing-audit-product');
          assert.equal(missing.status(),404);assert.equal(missing.headers()['set-cookie'],undefined);
          for (const route of ['/cart','/products/cart-simple']) {
            const head = await context.request.head(origin+prefix(locale)+route);
            assert.equal(head.status(),200);assert.equal(head.headers()['set-cookie'],undefined);
          }
          assert.equal(Number((await query('SELECT count(*) FROM guest_carts')).rows[0].count),sessionsBefore);

          await navigate('/products/cart-simple',locale);
          const assertIcon=require('./browser-icons.cjs').assertIcon;
          await assertIcon(page.locator('[data-cart-link] svg'));
          await assertIcon(page.getByRole('button',{name:locale==='it'?'Cerca':'Search',exact:true}).locator('svg'));
          { const shot = {path:path.join(output,`icons-header-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
          if(js) {
            const before=page.url();await page.locator('[data-cart-link]').click();
            await page.locator('#minicart-content [data-minicart-count="0"]').waitFor();
            assert.equal(page.url(),before);assert.ok(await page.locator('#minicart').isVisible());
            await compactButtons('#minicart .ui-button:not(.ui-button--mini)');
            await assertIcon(page.locator('#minicart-title svg'),24);
            const close=page.getByRole('button',{name:locale==='it'?'Chiudi il carrello':'Close cart',exact:true});
            const open=page.getByRole('link',{name:locale==='it'?'Apri il carrello completo':'Open full cart',exact:true});
            await assertIcon(close.locator('svg'),16); await assertIcon(open.locator('svg'),16);
            assert.equal((await close.textContent()).trim(), '', 'Minicart closes with the icon only');
            assert.deepEqual(await close.evaluate(e=>{const r=e.getBoundingClientRect();return [r.width,r.height];}),[28,28]);
            assert.equal(await close.getAttribute('title'),locale==='it'?'Chiudi il carrello':'Close cart');
            const checkoutLink=page.getByRole('link',{name:locale==='it'?'Vai al checkout':'Go to checkout',exact:true});
            assert.equal(await checkoutLink.getAttribute('href'),prefix(locale)+'/checkout');
            assert.equal(await page.locator('.minicart-footer button').count(),0);

            assert.ok(await close.evaluate(e=>document.activeElement===e),'Close keeps autofocus');
            { const shot = {path:path.join(output,`icons-minicart-empty-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
            const footer=await page.locator('.minicart-footer').evaluate(e=>{
              const [left,right]=Array.from(e.children,button=>button.getBoundingClientRect());
              return {sameRow:Math.abs(left.top-right.top)<1,equalWidth:Math.abs(left.width-right.width)<1,leftFirst:left.right<right.left};
            });
            assert.deepEqual(footer,{sameRow:true,equalWidth:true,leftFirst:true});
            await page.keyboard.press('Escape');
            // Native focus restoration precedes the dialog close event. Wait for
            // the component handler before starting another keyboard interaction.
            await page.waitForFunction(()=>document.querySelector('[data-cart-link]')?.getAttribute('aria-expanded')==='false');
            assert.ok(await page.locator('[data-cart-link]').evaluate(e=>document.activeElement===e));
          } else {
            await page.locator('[data-cart-link]').click();await page.waitForURL(origin+prefix(locale)+'/cart');
            assert.equal(await page.locator('#minicart').isVisible(),false);
            await navigate('/products/cart-simple',locale);
          }
          const quantity=page.getByLabel(locale==='it'?'Quantità':'Quantity',{exact:true});
          assert.equal(await quantity.inputValue(),'6');await quantity.focus();await page.keyboard.press('Tab');assert.ok(await page.getByRole('button',{name:add,exact:true}).evaluate(e=>document.activeElement===e),JSON.stringify(await page.evaluate(()=>({active:document.activeElement?.outerHTML.slice(0,250),dialog:document.querySelector('#minicart')?.open}))));
          await page.getByRole('button',{name:add,exact:true}).click();await page.waitForURL('**/cart?saved=1');
          const headerGeometry=await page.evaluate(()=>{const actions=document.querySelector('.store-header-actions').getBoundingClientRect(),cart=document.querySelector('[data-cart-link]').getBoundingClientRect(),languages=document.querySelector('.store-languages').getBoundingClientRect(),header=document.querySelector('.site-header').getBoundingClientRect();return {above:languages.bottom<=cart.top,right:Math.abs(languages.right-cart.right)<2,top:actions.top-header.top,centered:Math.abs((actions.top+actions.bottom-document.querySelector('.wordmark').getBoundingClientRect().top-document.querySelector('.wordmark').getBoundingClientRect().bottom)/2)<2};});
          assert.ok(headerGeometry.above&&headerGeometry.right,'Language selector is right-aligned above the cart');if(process.env.TALOCART_CART_TEST_THEME==='essential')assert.ok(headerGeometry.centered,'Essential header actions align with the wordmark');else assert.ok(headerGeometry.top<=24,'Cart/languages align to the top of the header');
          await page.locator('.store-languages a').last().focus();await page.keyboard.press('Tab');
          assert.ok(await page.locator('.store-header-actions a[href$="/account"]').evaluate(e=>document.activeElement===e),'Keyboard follows languages then customer account');
          await page.keyboard.press('Tab');
          assert.ok(await page.locator('[data-cart-link]').evaluate(e=>document.activeElement===e),'Keyboard follows customer account then cart');
          assert.equal(await page.locator('.site-header a[href="/admin"]').count(),0);assert.equal(await page.locator('.site-header a[href$="/catalog"]').count(),0);assert.equal(await page.locator('[data-cart-count]').innerText(),'6');assert.equal(await page.locator('[data-cart-total=gross]').innerText(),locale==='it'?'7,20 €':'7.20 €');
          const geometry = await page.locator('[data-cart-item]').first().evaluate(row => {
            const input = row.querySelector('input[name=quantity]').getBoundingClientRect();
            return [...row.querySelectorAll('button[name=action]')].map(button => {
              const rect = button.getBoundingClientRect();
              return {right: rect.left > input.right, aligned: Math.abs((rect.top + rect.bottom - input.top - input.bottom) / 2) < 2,
                iconOnly: !button.textContent.trim() && !!button.querySelector('svg'), named: !!button.getAttribute('aria-label'), compact: rect.width === 36 && rect.height === 36};
            });
          });
          assert.ok(geometry.length === 2 && geometry.every(g => g.right && g.aligned && g.iconOnly && g.named && g.compact), 'Compact icon actions align beside quantity');
          const checkout = page.locator('.cart-summary a[href$="/checkout"]');
          assert.ok(await checkout.evaluate(e => e.getBoundingClientRect().height >= 48), 'Checkout uses full size button');
          assert.ok(await page.locator('.cart-summary .ui-help').first().evaluate(e => parseFloat(getComputedStyle(e).marginTop) >= 16), 'Checkout guidance has top spacing');
          await page.locator('[data-cart-item] input[name=quantity]').focus();
          await page.keyboard.press('Tab');
          assert.ok(await page.locator('[data-cart-item] button[value=update]').evaluate(e => e === document.activeElement));
          await page.keyboard.press('Tab');
          assert.ok(await page.locator('[data-cart-item] button[value=remove]').evaluate(e => e === document.activeElement));
          { const shot = {path:path.join(output,`actions-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
          // Minimum belongs to the final line: a pack of three extends six to nine.
          await navigate('/products/cart-simple',locale);await page.getByLabel(locale==='it'?'Quantità':'Quantity',{exact:true}).fill('3');
          await page.getByRole('button',{name:add,exact:true}).click();await page.waitForURL('**/cart?saved=1');assert.equal(await page.locator('[data-cart-count]').innerText(),'9');
          await page.locator('[data-cart-item] input[name=quantity]').fill('6');await page.getByRole('button',{name:locale==='it'?'Aggiorna':'Update',exact:true}).click();await page.waitForURL('**/cart?saved=1');
          // Arrival from another site must preserve a pre-existing guest cart.
          await page.goto(`http://localhost:${port}/catalog`);
          await page.evaluate(href=>{const a=document.createElement('a');a.id='external-cart-link';a.href=href;a.textContent='Open saved cart';document.body.append(a);},origin+prefix(locale)+'/cart');
          await page.locator('#external-cart-link').click();await page.waitForURL(origin+prefix(locale)+'/cart');
          assert.equal(await page.locator('[data-cart-count]').innerText(),'6','External arrival preserves the cart');
          const row=page.locator('[data-cart-item]').first();
          // Force an invalid quantity through the real form to test server validation.
          await row.locator('input[name=quantity]').fill('7');await row.locator('form').evaluate(f=>{f.noValidate=true;});
          const rejected=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.includes('/cart/items/'));
          await row.getByRole('button',{name:locale==='it'?'Aggiorna':'Update',exact:true}).click();assert.equal((await rejected).status(),422);
          assert.equal(await page.locator('[data-cart-item] input[name=quantity]').inputValue(),'7');assert.equal(await page.locator('[data-cart-item] input[name=quantity]').getAttribute('aria-invalid'),'true');assert.ok(await page.locator('#cart-error').isVisible());
          const otherLocale=locale==='it'?'en':'it';
          await page.locator(`.store-languages a[lang=${otherLocale}]`).click();await page.waitForURL(origin+prefix(otherLocale)+'/cart');
          assert.equal(await page.locator('[data-cart-item] input[name=quantity]').inputValue(),'6','Language navigation never submits a rejected quantity');
          await page.locator(`.store-languages a[lang=${locale}]`).click();await page.waitForURL(origin+prefix(locale)+'/cart');
          await page.locator('[data-cart-item] input[name=quantity]').fill('9');await page.getByRole('button',{name:locale==='it'?'Aggiorna':'Update',exact:true}).click();await page.waitForURL('**/cart?saved=1');
          assert.equal(await page.locator('[data-cart-count]').innerText(),'9');
          // A saved form submitted twice must return 409, never merge again.
          await navigate('/products/cart-simple',locale);const captured=await page.locator('.store-cart-add').evaluate(f=>Object.fromEntries(new FormData(f)));
          const target=origin+prefix(locale)+'/cart/add';assert.equal((await context.request.post(target,{form:captured})).status(),200); // follows 303
          assert.equal((await context.request.post(target,{form:captured})).status(),409);
          // The real stale form must lead to an explicit reload, never another stale submit.
          const staleResponse = page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/cart/add'));
          await page.getByRole('button',{name:add,exact:true}).click();
          assert.equal((await staleResponse).status(),409);
          assert.equal(await page.locator('form[method=post]').count(),0);
          await compactButtons('main .ui-button:not(.cart-checkout-link)');
          const reload = page.locator('#cart-reload');
          await reload.focus();assert.ok(await reload.evaluate(e=>document.activeElement===e));
          { const shot = {path:path.join(output,`stale-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
          await reload.click();await page.waitForURL(origin+prefix(locale)+'/cart');
          assert.equal(await page.locator('[data-cart-count]').innerText(),'15');
          await navigate('/cart',locale);assert.equal(await page.locator('[data-cart-count]').innerText(),'15');
          // Prices and stock are persistent line state, never action toasts.
          const contextual = async locator => {
            assert.equal(await locator.getAttribute('data-ui-toast'),null);
            assert.equal(await locator.locator('[data-ui-toast-close]').count(),0);
          };
          await query('UPDATE catalog_products SET price_cents=150 WHERE id=$1',[simple]);
          await navigate('/cart',locale);assert.ok(await page.locator('[id^=cart-changed-]').isVisible());assert.equal(await page.locator('[data-cart-total=gross]').innerText(),locale==='it'?'27,00 €':'27.00 €');
          await query('UPDATE catalog_products SET stock=10 WHERE id=$1',[simple]);await navigate('/cart',locale);assert.ok(await page.locator('[id^=cart-issue-]').isVisible());assert.equal(await page.locator('[data-cart-total=gross]').count(),0);
          await contextual(page.locator('[id^=cart-issue-]'));
          await contextual(page.locator('#cart-total-unavailable'));
          await page.locator('[data-cart-item] input[name=quantity]').fill('6');await page.getByRole('button',{name:locale==='it'?'Aggiorna':'Update',exact:true}).click();await page.waitForURL('**/cart?saved=1');
          await query('UPDATE catalog_products SET stock=100 WHERE id=$1',[simple]);
          // Select a variant, including the HTMX path and the no-JS submit fallback.
          await navigate('/products/cart-variant',locale);assert.equal(await page.locator('.store-cart-add').count(),0);
          await page.locator('#store-value_1').selectOption(value);
          if(js){await page.waitForURL('**value_1=*');await page.locator('.store-cart-add').waitFor();}else{await page.getByRole('button',{name:locale==='it'?'Mostra variante':'Show variant',exact:true}).click();}
          assert.equal(await page.locator('[data-product-sku]').innerText(),'CART-VARIANT');await page.getByRole('button',{name:add,exact:true}).click();await page.waitForURL('**/cart?saved=1');assert.equal(await page.locator('[data-cart-item]').count(),2);
          if(js) {
            const before=page.url(),total=await page.locator('[data-cart-total=gross]').innerText(),net=await page.locator('[data-cart-total=net]').innerText(),tax=await page.locator('[data-cart-total=tax]').innerText();
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-content [data-minicart-count="7"]').waitFor();
            assert.equal(page.url(),before);assert.equal(await page.locator('.minicart-item').count(),2);
            assert.equal(await page.locator('[data-minicart-total]').innerText(),total);
            assert.equal(await page.locator('[data-minicart-net]').innerText(),net);
            assert.equal(await page.locator('[data-minicart-tax]').innerText(),tax);
            assert.deepEqual(await page.locator('.minicart-totals dt').allTextContents(),locale==='it'?['Imponibile','Imposte','Totale articoli']:['Net price','Taxes','Items total']);
            assert.deepEqual(await page.locator('.minicart-thumbnail img').evaluateAll(images=>images.map(i=>i.getAttribute('src'))),[`/media/image/${productImage}/thumbnail`,`/media/image/${productImage}/thumbnail`], 'Variant falls back to product image');
            for(const image of await page.locator('.minicart-thumbnail img').all()){
              await image.evaluate(i=>i.decode());
              assert.ok(await image.evaluate(i=>i.naturalWidth>0 && i.getBoundingClientRect().width<=64));
            }
            assert.equal(await page.locator('#minicart-content').getByText(locale==='it'?'Gli articoli nel carrello non prenotano le scorte.':'Items in the cart do not reserve stock.',{exact:true}).count(),0);
            assert.ok(!(await page.locator('#minicart-content').innerText()).includes(locale==='it'?'Le imposte sono stimate':'Taxes are estimated'));
            const density=await page.locator('.minicart-item-copy').first().evaluate(e=>{
              const heading=getComputedStyle(e.querySelector('h3')),detail=getComputedStyle(e.querySelector('.ui-help')),quantity=getComputedStyle(e.querySelector('.minicart-quantity'));
              return {heading:parseFloat(heading.lineHeight)/parseFloat(heading.fontSize),detail:parseFloat(detail.marginTop),quantity:parseFloat(quantity.marginTop)};
            });
            assert.ok(density.heading<=1.3 && density.detail<=2 && density.quantity<=2, 'Product details keep compact line spacing');

            assert.ok(await page.locator('#minicart-content').innerText().then(t=>t.includes('Size: Large')));
            for(const key of ['Tab','Shift+Tab']) for(let n=0;n<9;n++){await page.keyboard.press(key);assert.ok(await page.locator('#minicart').evaluate(e=>e.contains(document.activeElement)),await page.evaluate(()=>document.activeElement.outerHTML.slice(0,400)));}
            assert.ok(await page.locator('#minicart').evaluate(e=>e.scrollWidth<=e.clientWidth));
            assert.equal(await page.evaluate(()=>getComputedStyle(document.documentElement).overflow),'hidden','Modal keeps the background from scrolling');
            await require('./ui-contract.cjs')(page);
            { const shot = {path:path.join(output,`minicart-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
            await page.locator('.minicart-footer a[href$="/checkout"]').click();
            await page.waitForURL(origin+prefix(locale)+'/checkout');
            await navigate('/cart',locale);
            assert.equal(await page.locator('[data-cart-item]').count(),2,'Checkout navigation preserves cart lines');
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-content [data-minicart-count="7"]').waitFor();
            await page.mouse.click(1,1);assert.equal(await page.locator('#minicart').isVisible(),false);
            assert.ok(await page.locator('[data-cart-link]').evaluate(e=>document.activeElement===e));
            await query('UPDATE catalog_variants SET image_key=$1 WHERE product_id=$2',[variantImageKey,variantProduct]);
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-content [data-minicart-count="7"]').waitFor();
            assert.equal(await page.locator('.minicart-thumbnail img').last().getAttribute('src'),`/media/image/${variantImage}/thumbnail`);
            await page.keyboard.press('Escape');
            await query('UPDATE catalog_variants SET image_key=NULL WHERE product_id=$1',[variantProduct]);
            // Reopening refreshes data rather than using the previous fragment.
            await query('UPDATE catalog_products SET stock=1 WHERE id=$1',[simple]);
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-unavailable').waitFor();await contextual(page.locator('#minicart-unavailable'));
            assert.equal(await page.locator('[data-minicart-total]').count(),0);
            await page.locator('[data-minicart-close]').first().click();
            await query('UPDATE catalog_products SET stock=100 WHERE id=$1',[simple]);
            let releaseRequest;
            const pendingRequest=new Promise(resolve=>{releaseRequest=resolve;});
            await page.route('**/cart?preview=1',route=>releaseRequest(route));
            await page.evaluate(()=>{window.minicartAborted=false;document.addEventListener('htmx:sendAbort',()=>{window.minicartAborted=true;},{once:true});});
            await page.locator('[data-cart-link]').click();const blockedRequest=await pendingRequest;
            assert.ok(await page.locator('#minicart-loading').isVisible());
            await page.locator('[data-minicart-close]').first().click();await page.waitForFunction(()=>window.minicartAborted);
            await blockedRequest.abort();await page.unroute('**/cart?preview=1');
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-content [data-minicart-count="7"]').waitFor();
            await page.keyboard.press('Escape');
            await page.route('**/cart?preview=1',route=>route.fulfill({status:503,body:'Temporary failure'}));
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-error').waitFor();
            assert.equal(await page.locator('#minicart-content').innerText(),'');
            assert.equal(await page.locator('#minicart-error[data-ui-toast] [role=alert]').count(),1);
            assert.equal(await page.locator('#minicart [data-ui-toast-viewport]').count(),1,'Modal notifications stay in the top layer');
            const stack=await page.locator('#minicart [data-ui-toast-viewport]').boundingBox();
            const heading=await page.locator('.minicart-heading').boundingBox();
            assert.ok(stack.y+stack.height<=heading.y,'Notifications reserve space above modal controls');
            await page.locator('#minicart').screenshot({animations:'disabled',path:path.join(output,`toast-error-${name}-${locale}.png`)});
            await page.locator('#minicart-error button').focus(); await page.keyboard.press('Enter');
            assert.equal(await page.locator('#minicart-error').isVisible(),false);
            assert.ok(await page.locator('[data-minicart-close]').first().evaluate(e=>e===document.activeElement));
            await page.keyboard.press('Escape');
            await page.locator('[data-cart-link]').click(); await page.locator('#minicart-error').waitFor();
            await page.locator('#minicart-error button').click();
            assert.equal(await page.locator('#minicart-error').isVisible(),false,'Reusable error can be dismissed on a later failure');
            await page.keyboard.press('Escape');
            await page.locator('[data-cart-link]').click(); await page.locator('#minicart-error').waitFor();
            await page.locator('.minicart-footer a[href$="/cart"]').click();await page.waitForURL(origin+prefix(locale)+'/cart');
            await page.unroute('**/cart?preview=1');
          }
          await require('./ui-contract.cjs')(page);{ const shot = {path:path.join(output,`${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          if(js) {
            await navigate(`/products/cart-variant?value_1=${value}`,locale);
            await page.locator('#cart-quantity').focus();{ const shot = {path:path.join(output,`variant-${name}-${locale}.png`),fullPage:true}; await presentation.verify(page, path.basename(shot.path)); await presentation.screenshot(page, shot); }
            await page.evaluate(()=>{window.cartSettled=false;document.body.addEventListener('htmx:afterSettle',()=>{window.cartSettled=true;},{once:true});});
            const changed=page.waitForResponse(r=>r.request().headers()['hx-request']==='true');
            await page.locator('#store-value_1').selectOption(value);await changed;await page.waitForFunction(()=>window.cartSettled);
            assert.deepEqual(cspErrors,[],'Repeated variant swaps respect CSP');
          }

          const cookies=await context.cookies();const guestCookie=cookies.find(c=>c.name==='talocart_cart');assert.ok(guestCookie.httpOnly);assert.equal(guestCookie.sameSite,'Lax');assert.equal(guestCookie.secure,false);
          await stop();
          await query("INSERT INTO guest_carts(token_hash,csrf,expires_at) SELECT repeat(md5('cart-maintenance-'||i),2),repeat('a',64),now()-interval '1 day' FROM generate_series(1,205) i");
          await start();
          for(let attempt=0;attempt<100;attempt++) {
            if(Number((await query('SELECT count(*) FROM guest_carts WHERE expires_at<=now()')).rows[0].count)===0)break;
            await new Promise(resolve=>setTimeout(resolve,100));
          }
          assert.equal(Number((await query('SELECT count(*) FROM guest_carts WHERE expires_at<=now()')).rows[0].count),0,'Startup drains multiple expired batches');
          await navigate('/cart',locale);assert.equal(await page.locator('[data-cart-item]').count(),2);
          await compactButtons('main .ui-button:not(.cart-checkout-link)');
          const other=await browser.newContext({javaScriptEnabled:js});const stranger=await other.newPage();await stranger.goto(origin+prefix(locale)+'/cart');assert.equal(await stranger.locator('[data-cart-item]').count(),0);await other.close();
          // Retired lines are retained and removable without browser numeric validation.
          await query("UPDATE catalog_products SET status='draft' WHERE id=$1",[simple]);await navigate('/cart',locale);assert.ok(await page.locator('[id^=cart-issue-]').isVisible());
          await page.locator('[data-cart-item]').first().getByRole('button',{name:locale==='it'?'Rimuovi':'Remove',exact:true}).click();await page.waitForURL('**/cart?saved=1');assert.equal(await page.locator('[data-cart-item]').count(),1);
          if(js) {
            // The minicart removes a single line in place; closing it refreshes the stale cart page.
            await page.evaluate(()=>{window.minicartSettled=false;document.body.addEventListener('htmx:afterSettle',()=>{window.minicartSettled=true;},{once:true});});
            await page.locator('[data-cart-link]').click();await page.locator('#minicart-content [data-minicart-count]:not([data-minicart-count="0"])').waitFor();await page.waitForFunction(()=>window.minicartSettled);
            const remove=page.getByRole('button',{name:new RegExp(`^${locale==='it'?'Rimuovi':'Remove'} `)});
            assert.equal(await remove.count(),1);assert.equal(await page.locator('#minicart form[method=post]').count(),1);
            await remove.focus();await page.keyboard.press('Enter');
            await page.locator('#minicart-content [data-minicart-count="0"]').waitFor();
            assert.equal(await page.locator('#minicart-content [role=status]').textContent(),locale==='it'?'Articolo rimosso dal carrello.':'Item removed from your cart.');
            assert.ok(await page.locator('[data-minicart-close]').first().evaluate(e=>e===document.activeElement),'Focus moves to close when the list empties');
            assert.equal(await page.locator('[data-cart-count]').innerText(),'0');
            const refreshed=page.waitForEvent('load');await page.keyboard.press('Escape');await refreshed;
            assert.equal(await page.locator('[data-cart-item]').count(),0);
          } else {
            await page.getByRole('button',{name:locale==='it'?'Rimuovi':'Remove',exact:true}).click();await page.waitForURL('**/cart?saved=1');assert.equal(await page.locator('[data-cart-item]').count(),0);assert.equal(await page.locator('[data-cart-count]').innerText(),'0');
          }
          await compactButtons('main .ui-button:not(.cart-checkout-link)');
          await query("UPDATE catalog_products SET status='published',price_cents=100,stock=100 WHERE id=$1",[simple]);
          if(js) await require('./browser-minicart-checkout.cjs')(page, navigate, locale);
          results.push({profile:name,locale,passed:true});
        }
        assert.deepEqual(errors,[]);assert.deepEqual(cspErrors,[]);
      } finally {await stop();await context.close();await db.query('SET search_path TO public');await db.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);fs.rmSync(folder,{recursive:true,force:true});}
    }
  } finally {await browser.close();await db.end();}
  fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));console.log(JSON.stringify(results));
})().catch(e=>{console.error(e);process.exitCode=1;});
