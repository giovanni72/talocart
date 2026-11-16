// Shared rich-text editor and public content, using synthetic data only.
const assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {randomBytes}=require('node:crypto'),{spawn,spawnSync}=require('node:child_process'),{once}=require('node:events');
const {Client}=require('pg'),{chromium}=require('playwright');
(async()=>{
 assert.ok(process.env.TALOCART_TEST_DATABASE_URL,'Dedicated test database required');
 const db=new Client({connectionString:process.env.TALOCART_TEST_DATABASE_URL});await db.connect();
 const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
 const schema='talocart_rich_text_'+process.pid+'_'+Date.now(),folder=fs.mkdtempSync(path.join(os.tmpdir(),'talocart-rich-text-'));
 const origin='http://127.0.0.1:3147',output=path.resolve('.local/verification/rich-text');fs.mkdirSync(output,{recursive:true});
 const url=new URL(process.env.TALOCART_TEST_DATABASE_URL);url.searchParams.set('options','-csearch_path='+schema);
 const config=path.join(folder,'talocart.toml');fs.writeFileSync(config,'[server]\nlisten="127.0.0.1:3147"\n',{mode:0o600});
 const binary=path.resolve(process.env.TALOCART_TEST_BINARY||'target/debug/talocart');
 const env={...process.env,TALOCART_DATABASE_URL:url.href,TALOCART_LISTEN:'127.0.0.1:3147',TALOCART_MEDIA_ROOT:path.join(folder,'media'),TALOCART_LOG:'warn'};
 let server;const results=[];
 const start=async()=>{const fd=fs.openSync(path.join(output,'server.log'),'a',0o600);server=spawn(binary,['--config',config,'serve'],{env,stdio:['ignore',fd,fd]});fs.closeSync(fd);for(let i=0;i<150;i++){try{if((await fetch(origin+'/health/ready')).ok)return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('Readiness');};
 const stop=async()=>{if(server&&server.exitCode===null){const done=once(server,'exit');server.kill('SIGTERM');await done;}};
 try{
  await db.query('CREATE SCHEMA '+schema);await db.query('SET search_path TO '+schema);
  assert.equal(spawnSync(binary,['--config',config,'migrate'],{env,stdio:'pipe'}).status,0,'Clean migration');await start();
  const setup=await browser.newContext(),admin=await setup.newPage(),password=randomBytes(20).toString('hex');
  await setup.addCookies([{name:'talocart_admin_language',value:'it',url:origin}]);await admin.goto(origin+'/setup');
  await admin.getByLabel('Nome del negozio',{exact:true}).fill('Rich text editors');await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);
  await admin.getByRole('button',{name:'Crea negozio',exact:true}).click();await admin.waitForURL('**/admin/login');
  await admin.getByLabel('Email amministratore').fill('owner@example.test');await admin.getByLabel('Password',{exact:true}).fill(password);await admin.getByRole('button',{name:'Accedi',exact:true}).click();await admin.waitForURL('**/admin');
  const cookies=(await setup.cookies()).filter(c=>c.name==='talocart_session');
  await db.query("INSERT INTO storefront_languages(code) VALUES('en'),('it') ON CONFLICT DO NOTHING");
  await db.query("UPDATE storefront_settings SET primary_locale='en' WHERE id=1");
  const tax = (await db.query('SELECT id FROM tax_classes ORDER BY id LIMIT 1')).rows[0].id;
  const old = 'Literal <b>tags</b> & text.\nSecond line.\n\nNext paragraph.';
  const product = (await db.query("INSERT INTO catalog_products(name,model,slug,price_cents,status,tax_class_id,description) VALUES('Rich product','RICH','rich-product',100,'published',$1,$2) RETURNING id", [tax,old])).rows[0].id;
  const pageId = (await db.query("INSERT INTO information_pages(slug,name,description,published) VALUES('rich-page','Rich page',$1,true) RETURNING id",[old])).rows[0].id;
  const raw = '<h2>Features</h2><p>Soft <strong>cotton</strong> &amp; <u>comfort</u>.</p><ul><li>One</li><li>Two</li></ul><blockquote><p>Quote</p></blockquote><p><a href="/contact">Contact</a></p>';
  const translated = '<h3>Dettagli</h3><p>Testo <em>italiano</em>.</p><table><caption>Misure</caption><thead><tr><th scope="col">Taglia</th><th scope="col">Torace</th></tr></thead><tbody><tr><th scope="row">S</th><td>50</td></tr></tbody></table>';
  for (const theme of ['default','essential']) {
   await db.query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[theme]);
   for (const locale of ['en','it']) for (const [profile,width,js] of [['desktop',1440,true],['mobile',390,true],['no-js',1440,false],['mobile-no-js',390,false]]) {
    await db.query('UPDATE administrators SET locale=$1',[locale]);
    const context=await browser.newContext({viewport:{width,height:1000},javaScriptEnabled:js});await context.addCookies(cookies);
    const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
    try {
     for (const [kind,id,url,publicURL,saveSelector] of [
      ['product','product-description','/admin/catalog/products/'+product,'/products/rich-product','button[type=submit][form=product-editor-form]'],
      ['page','page-description','/admin/pages/'+pageId,'/pages/rich-page','button[value=save]']]) {
      await page.goto(origin+url);
      const primary=page.locator('[data-rich-text]').filter({has:page.locator('#'+id)});
      const secondaryId=kind==='product'?'product-content-it-description':'page-content-it-description';
      const secondary=page.locator('[data-rich-text]').filter({has:page.locator('#'+secondaryId)});
      assert.equal(await primary.count(),1);assert.equal(await secondary.count(),1);
      const save = async(expected=303)=>{
       const response=page.waitForResponse(r=>r.request().method()==='POST'&&r.url().includes(new URL(origin+url).pathname));
       // Product form uses the existing sticky save bar.
       const submit=kind==='product'?page.locator('button[form="product-editor-form"][type=submit]').first():page.locator(saveSelector);
       await submit.click();assert.equal((await response).status(),expected);await page.waitForLoadState('load');
      };
      // Opening and submitting untouched legacy content must preserve literal text (native forms serialize LF as CRLF).
      await db.query(`UPDATE ${kind==='product'?'catalog_products':'information_pages'} SET description=$1 WHERE id=$2`,[old,kind==='product'?product:pageId]);
      await db.query(`DELETE FROM ${kind==='product'?'catalog_products':'information_pages'}_translations WHERE entity_id=$1`,[kind==='product'?product:pageId]);
      await page.reload();
      if(js){
       assert.equal(await primary.locator('[data-rich-command=undo]').isDisabled(),true,'No undo before the first edit');
       assert.equal(await primary.locator('[data-rich-command=redo]').isDisabled(),true,'No redo before the first edit');
       const controls=await primary.locator('.ui-rich-tool:visible').evaluateAll(nodes=>nodes.map(node=>({text:node.textContent.trim(),label:node.getAttribute('aria-label'),tooltip:node.title,icons:node.querySelectorAll('svg').length,width:node.getBoundingClientRect().width,height:node.getBoundingClientRect().height,border:getComputedStyle(node).borderTopWidth,ring:getComputedStyle(node,'::after').content})));
       assert.ok(controls.length>=16);
       for(const control of controls){assert.equal(control.text,'');assert.ok(control.label);assert.equal(control.tooltip,control.label);assert.equal(control.icons,1);assert.ok(control.width<=32&&control.height<=32);assert.equal(control.border,'0px');assert.equal(control.ring,'none');}
       assert.equal(await primary.locator('.ui-rich-source').getAttribute('aria-describedby'),await page.locator('#'+id).getAttribute('aria-describedby'));
       await primary.locator('[data-rich-view=html]').click();
       await primary.locator('[data-rich-view=visual]').click();
      }
      await save();
      assert.equal((await db.query(`SELECT description FROM ${kind==='product'?'catalog_products':'information_pages'} WHERE id=$1`,[kind==='product'?product:pageId])).rows[0].description.replaceAll('\r\n','\n'),old);
      if(js){
       await primary.locator('[data-rich-view=html]').focus();await page.keyboard.press('ArrowLeft');
       assert.equal(await primary.locator('[data-rich-view=visual]').getAttribute('aria-selected'),'true');
       await primary.locator('[data-rich-view=html]').click();
       await primary.locator('.ui-rich-source').fill('<p class="custom">Keep this source</p>');
       await primary.locator('[data-rich-view=visual]').click();
       assert.equal(await primary.locator('.ui-rich-source').inputValue(),'<p class="custom">Keep this source</p>');
       assert.equal(await primary.locator('[data-rich-view=html]').getAttribute('aria-selected'),'true');
       assert.ok(await primary.locator('[data-rich-error]').innerText());
       await primary.locator('.ui-rich-source').fill(raw);await primary.locator('[data-rich-view=visual]').click();
       assert.equal(await primary.locator('.tiptap strong').innerText(),'cotton');
       await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-${kind}-visual.png`),fullPage:true});
       await primary.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-${kind}-toolbar.png`)});
       if(profile==='desktop'&&locale==='en'&&theme==='default'){
        await primary.locator('.tiptap p').first().click();
        await primary.locator('[data-rich-link-open]').click();
        await primary.locator('[data-rich-link-url]').fill('javascript:bad()');await page.keyboard.press('Enter');
        assert.ok(await primary.locator('[data-rich-error]').innerText());
        await primary.locator('[data-rich-link-url]').fill('/contact');await page.keyboard.press('Enter');
        await page.keyboard.type(' linked ');
        assert.ok(await primary.locator('.tiptap a[href="/contact"]').count());
        await primary.locator('[data-rich-link-open]').click();await page.keyboard.press('Escape');
        assert.equal(await primary.locator('.ui-rich-link').isVisible(),false);
       }
       await primary.locator('[data-rich-command=bold]').click();await page.keyboard.type('X');
       assert.equal(await primary.locator('[data-rich-command=undo]').isEnabled(),true,'Editing enables undo');
       await primary.locator('[data-rich-command=undo]').click();
       assert.equal(await primary.locator('[data-rich-command=redo]').isEnabled(),true,'Undo enables redo');
       await secondary.locator('[data-rich-view=html]').click();await secondary.locator('.ui-rich-source').fill(translated);
       // Keep primary source exact if the visual content was not changed.
       await primary.locator('[data-rich-view=html]').click();
       await primary.locator('.ui-rich-source').fill('x'.repeat(kind==='product'?4000:20000));
       if(kind==='product')await page.locator('#tab-commerce').click();
       await save(422);
       assert.equal(await page.locator('#'+id).inputValue(),'x'.repeat(kind==='product'?4000:20000));
       assert.equal(await page.locator('#'+id).getAttribute('aria-invalid'),'true');
       if(kind==='product')assert.equal(await page.locator('[data-product-tabs]').getAttribute('data-active-tab'),'general');
       await primary.locator('[data-rich-view=html]').click();
       for(const unsupported of ['<p><a href="/contact" title="Keep this title">Contact</a></p>','<html lang="it"><body><p>Keep this document</p></body></html>']){
        await primary.locator('.ui-rich-source').fill(unsupported);await primary.locator('[data-rich-view=visual]').click();
        assert.equal(await primary.locator('[data-rich-view=html]').getAttribute('aria-selected'),'true','Unsupported HTML must stay in source view');
        assert.equal(await primary.locator('.ui-rich-source').inputValue(),unsupported);
       }
       await primary.locator('.ui-rich-source').fill(raw);
      }else{
       await page.locator('#'+id+'-format').selectOption('html');await page.locator('#'+id).fill(raw+'<script>window.__richInjected=true</script>');
       await page.locator('#'+secondaryId+'-format').selectOption('html');await page.locator('#'+secondaryId).fill(translated);
      }
      if(kind==='page'){
       const slug=await page.locator('#page-slug').inputValue();await page.locator('#page-slug').fill('invalid/slug');await save(422);
       assert.equal(await page.locator('#'+secondaryId).inputValue(),translated);
       assert.ok((await page.locator('#'+id).inputValue()).includes('<strong>cotton</strong>'));
       await page.locator('#page-slug').fill(slug);
      }
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'No overflow');
      await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-${kind}-editor.png`),fullPage:true});
      await save();
      // Tables remain editable in source mode and survive an untouched resubmission.
      await page.goto(origin+url);
      if(js){
       assert.equal(await secondary.locator('[data-rich-view=html]').getAttribute('aria-selected'),'true');
       assert.ok((await secondary.locator('.ui-rich-source').inputValue()).includes('<table>'));
       await secondary.locator('[data-rich-view=visual]').click();
       assert.equal(await secondary.locator('[data-rich-view=html]').getAttribute('aria-selected'),'true');
      }else assert.ok((await page.locator('#'+secondaryId).inputValue()).includes('<table>'));
      await save();
      if(kind==='product'){
       await page.goto(origin+'/admin/catalog/products');
       const panel=page.locator('#product-description-'+product);
       const trigger=page.locator('summary[aria-controls="product-description-'+product+'"]');
       await trigger.focus();await page.keyboard.press('Enter');
       await panel.waitFor({state:'visible'});
       assert.equal((await panel.innerText()).trim(),'Features Soft cotton & comfort. One Two Quote Contact');
       assert.equal(await panel.locator('*').count(),0,'Tooltip stays escaped plain text');
       await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-product-tooltip.png`),fullPage:true});
      }
      await page.goto(origin+publicURL);
      const content=page.locator(kind==='product'?'.product-description':'.information-body');
      assert.equal(await content.locator('strong').innerText(),'cotton');assert.equal(await content.locator('li').count(),2);
      assert.equal(await page.evaluate(()=>window.__richInjected),undefined);
      if(kind==='product')assert.ok(!(await page.locator('meta[name=description]').getAttribute('content')).includes('<'));
      await page.goto(origin+'/it'+publicURL);assert.equal(await page.locator('.ui-rich-content em').innerText(),'italiano');
      assert.equal(await page.locator('.ui-rich-content table th[scope=col]').count(),2);
      assert.equal(await page.locator('.ui-rich-content table td').innerText(),'50');
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Table stays within mobile content');
      await page.screenshot({path:path.join(output,`${theme}-${locale}-${profile}-${kind}-public.png`),fullPage:true});
     }
     assert.deepEqual(errors,[]);results.push({theme,locale,profile,passed:true});console.log(theme,locale,profile,'passed');
    }finally{await context.close();}
   }
  }
  await setup.close();fs.writeFileSync(path.join(output,'results.json'),JSON.stringify(results,null,2));
 }finally{await stop();await browser.close();await db.query('DROP SCHEMA IF EXISTS '+schema+' CASCADE');await db.end();fs.rmSync(folder,{recursive:true,force:true});}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
