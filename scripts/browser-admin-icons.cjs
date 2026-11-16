// Real admin navigation/edit journeys, in the demo suite's dedicated schema.
const assert=require('node:assert/strict');
const path=require('node:path');
const {assertIcon}=require('./browser-icons.cjs');
module.exports=async({page,origin,query,output,name,js})=>{
 const identityResult=require('node:child_process').spawnSync(path.resolve(process.env.TALOCART_TEST_BINARY||'target/release/talocart'),['build-info'],{encoding:'utf8'});
 assert.equal(identityResult.status,0,'Compiled identity is available without Git or database queries');
 const identity=JSON.parse(identityResult.stdout);
 const viewport=page.viewportSize();
 // One synthetic zone, committed with its required country; no application data.
 await query('BEGIN');
 try{
  const zone=(await query("INSERT INTO tax_zones(name) VALUES('Icon test zone') RETURNING id")).rows[0].id;
  await query("INSERT INTO tax_zone_countries(country_code,zone_id) VALUES('US',$1)",[zone]);
  await query('COMMIT');
 }catch(e){await query('ROLLBACK');throw e;}
 const product=(await query("SELECT id FROM catalog_products WHERE model='SHIRT'")).rows[0].id;
 const routes=['/admin/catalog/products',`/admin/catalog/products/${product}/variants`,'/admin/catalog/categories','/admin/catalog/brands','/admin/settings/taxes/rates','/admin/settings/taxes/classes','/admin/settings/taxes/zones'];
 for(const locale of ['en','it']){
  await page.goto(origin+'/admin/account');
  const previous=await page.locator('html').getAttribute('lang');
  await page.locator('#account-language').selectOption(locale);
  await page.getByRole('button',{name:previous==='it'?'Salva lingua':'Save language',exact:true}).click();
  await page.waitForURL('**/admin/account?saved=1');
  // The menu glyphs share the section label's left edge at both sidebar widths.
  await page.goto(origin+'/admin/catalog/options');
  const version=page.locator('.sidebar-version');
  assert.equal(await version.getAttribute('data-build-version'),identity.version);
  assert.equal((await version.innerText()).replace(/\s+/g,' ').trim(),`${locale==='it'?'Versione':'Version'} ${identity.display}`);
  assert.equal(await page.locator('.sidebar-bottom a,.sidebar-bottom button,.sidebar-bottom [tabindex]').count(),0,'Version adds no focus stops');
  const navigation = await page.locator('.admin-sidebar nav').evaluate(nav => ({
   order: [...nav.children].map(item => new URL((item.matches('a') ? item : item.querySelector('a')).href).pathname),
   icons: [...nav.querySelectorAll('a,summary')].map(item => ({name:item.querySelector('svg')?.dataset.uiIcon,geometry:item.querySelector('svg')?.innerHTML,path:item.matches('a')?new URL(item.href).pathname:null})),
  }));
  assert.deepEqual(navigation.order, ['/admin','/admin/orders','/admin/customers','/admin/catalog/products','/admin/pages','/admin/media','/admin/settings','/admin/users','/admin/operations','/admin/components'], 'Sidebar follows store operating priorities');
  for(const paths of [['/admin/pages','/admin/banners'],['/admin/operations','/admin/updates','/admin/error-logs']]){
   const children=await page.locator('.sidebar-group').filter({has:page.locator(`a[href="${paths[0]}"]`)}).locator('.sidebar-submenu a').evaluateAll(items=>items.map(item=>new URL(item.href).pathname));
   assert.deepEqual(children,paths,'Design and maintenance keep their documented grouped links');
  }
  const catalogue = await page.locator('.sidebar-submenu a[href^="/admin/catalog/"]').evaluateAll(items=>items.map(item=>new URL(item.href).pathname));
  assert.deepEqual(catalogue, ['products','categories','brands','options','attributes'].map(key=>'/admin/catalog/'+key));
  const settings = await page.locator('.sidebar-submenu a[href^="/admin/settings"]').evaluateAll(items=>items.map(item=>new URL(item.href).pathname));
  assert.deepEqual(settings, ['/admin/settings',...['customer-fields','payments','shipping','coupons','order-statuses','maintenance','email','taxes/rates','taxes/classes','taxes/zones'].map(key=>'/admin/settings/'+key)]);
  assert.ok(navigation.icons.every(icon=>icon.name&&icon.geometry), 'Every menu item has an icon');
  const grouped=new Map();
  for(const icon of navigation.icons){const group=grouped.get(icon.name)||[];group.push(icon);grouped.set(icon.name,group);}
  assert.deepEqual(Object.fromEntries([...grouped].filter(([,icons])=>icons.length>1).map(([name,icons])=>[name,icons.map(i=>i.path).sort()])),{users:['/admin/customer-groups','/admin/settings/customer-fields'],'panels-top-left':['/admin/banners',null],'sliders-horizontal':['/admin/settings','/admin/settings/maintenance',null]},'Only documented group and configuration entries share icons');
  const uniqueIcons=[...grouped.values()].map(icons=>icons[0]);
  assert.equal(new Set(uniqueIcons.map(icon=>icon.geometry)).size,uniqueIcons.length,'Distinct icon names have distinct drawings');
  const density=await page.locator('body').getAttribute('data-density');
  for(const spacing of ['compact','comfortable']){
   await page.locator('body').evaluate((body,value)=>{body.dataset.density=value;},spacing);
   for(const width of [320,390,650,651,768,950,951,1440]){
    await page.setViewportSize({width,height:1050});
    const sidebar=await page.locator('.admin-sidebar').boundingBox();
    assert.ok(await version.isVisible(),`${locale} ${width}: version remains visible including mobile/no-JS`);
    const versionBox=await version.boundingBox(),navBox=await page.locator('.admin-sidebar nav').boundingBox();
    assert.ok(versionBox.y>=navBox.y+navBox.height-1,'Version follows all navigation entries');
    assert.ok(versionBox.x>=sidebar.x&&versionBox.x+versionBox.width<=sidebar.x+sidebar.width+.1,'Version fits inside the sidebar');
    for(const item of await page.locator('.admin-sidebar nav a:visible,.admin-sidebar nav summary:visible').all()){
     const box=await item.boundingBox();
     assert.ok(box.x>=sidebar.x&&box.x+box.width<=sidebar.x+sidebar.width+.01,'Menu stays inside sidebar');
    }
    if(width>650){
     const label=await page.locator('.sidebar-label').boundingBox();
     const padding=await page.locator('.admin-sidebar').evaluate(e=>({left:parseFloat(getComputedStyle(e).paddingLeft),right:parseFloat(getComputedStyle(e).paddingRight)}));
     assert.ok(padding.left<=18&&padding.right<=18,'Reduced sidebar side padding');
     for(const icon of await page.locator('.admin-sidebar nav > a > svg,.admin-sidebar nav > details > summary > svg').all()){
      assert.ok(Math.abs((await icon.boundingBox()).x-label.x)<.5,`${locale} ${width} ${spacing}: menu aligns with section label`);
     }
     const child=await page.locator('.sidebar-submenu a.active svg').boundingBox();
     assert.ok(child.x>label.x+16,'Submenu keeps its hierarchy');
    }
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Sidebar does not overflow viewport');
    if([390,768,1440].includes(width))await page.screenshot({path:path.join(output,`sidebar-spacing-${name}-${locale}-${spacing}-${width}.png`),fullPage:true});
   }
  }
  await page.locator('body').evaluate((body,value)=>{body.dataset.density=value;},density);
  await page.setViewportSize(viewport);
  for(const route of routes){
   await page.goto(origin+route);assert.equal(await page.locator('html').getAttribute('lang'),locale);
   const nav=page.locator('.admin-sidebar nav');
   for(const details of await nav.locator('details').all()){
    if(!await details.evaluate(e=>e.open)){
     await details.locator('summary').focus();await page.keyboard.press('Enter');
     assert.ok(await details.evaluate(e=>e.open),'Native menu opens from the keyboard');
    }
   }
   for(const item of await nav.locator('a,summary').all())await assertIcon(item.locator('svg'),16);
   assert.equal(await nav.locator('[aria-current=page]').count(),1);
   assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${locale} ${route} fits viewport`);
   const edits=page.locator('[data-ui-edit]');assert.ok(await edits.count(),`${route} has edit actions`);
   for(const edit of await edits.all()){
    assert.equal((await edit.innerText()).trim(),'','Edit has only the pencil');
    const label=await edit.getAttribute('aria-label');
    assert.ok(label.startsWith(locale==='it'?'Modifica ':'Edit '));
    assert.equal(await edit.getAttribute('title'),label);
    await assertIcon(edit.locator('svg'),16);
    const box=await edit.boundingBox();assert.equal(box.width,36);assert.equal(box.height,36);
   }
   const edit=edits.first(),href=await edit.getAttribute('href');
   await edit.focus();assert.equal(await edit.evaluate(e=>getComputedStyle(e).outlineStyle),'solid');
   const slug=route.split('/').pop();await page.screenshot({path:path.join(output,`admin-icons-${slug}-${name}-${locale}.png`),fullPage:true});
   await page.keyboard.press('Enter');await page.waitForURL(origin+href);
   assert.equal(await page.locator('form[method=post] input[name=csrf]').count()>0,true);
   const save=page.locator('main .ui-button:visible').filter({has:page.locator('svg[data-ui-icon=save]')}).first();
   assert.ok(await save.count()&&await save.isVisible(),'Editor retains its labelled save action');
   assert.ok((await save.innerText()).trim().length>0);
   await require('./ui-contract.cjs')(page);
   if(route==='/admin/catalog/products')await page.screenshot({path:path.join(output,`admin-icons-editor-${name}-${locale}.png`),fullPage:true});
  }
  await page.goto(origin+'/admin/catalog/products');
  const filter=page.getByRole('button',{name:locale==='it'?'Filtra':'Filter',exact:true});
  const reset=page.getByRole('link',{name:locale==='it'?'Azzera filtri':'Reset filters',exact:true});
  await assertIcon(filter.locator('svg'),16);await assertIcon(reset.locator('svg'),16);
  const [a,b]=await Promise.all([filter.boundingBox(),reset.boundingBox()]);
  assert.ok(Math.abs(a.y-b.y)<1&&a.x+a.width<=b.x,'Filter icon/text and reset stay on the same row');
  await page.getByLabel(locale==='it'?'Cerca per nome, modello o slug':'Search by name, model or slug',{exact:true}).fill('no-icon-result');
  await filter.click();await page.waitForURL(url=>url.searchParams.get('q')==='no-icon-result');
  await reset.click();await page.waitForURL(url=>url.pathname==='/admin/catalog/products'&&!url.searchParams.has('q'));
  await page.goto(origin+'/admin/media');
  for(const glyph of ['upload','folder-plus','refresh-cw'])assert.ok(await page.locator(`main .ui-button svg[data-ui-icon="${glyph}"]`).count());
  await page.screenshot({path:path.join(output,`admin-icons-media-${name}-${locale}.png`),fullPage:true});
 }
 await page.setViewportSize(viewport);
 await page.goto(origin+'/admin');
};
