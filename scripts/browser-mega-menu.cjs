// Runs inside browser-demo's isolated database schema and media root.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,query,origin,output,name,js,viewport}) => {
  const create = async (label, slug, parent = null, order = 0, active = true) =>
    (await query('INSERT INTO catalog_categories(name,slug,parent_id,sort_order,active) VALUES ($1,$2,$3,$4,$5) RETURNING id', [label,slug,parent,order,active])).rows[0].id;
  const root = await create('Abbigliamento · Collezioni', 'mega-test-root');
  const branches = [];
  const menu = page.locator('.category-navigation');
  const dropdown = page.locator('.category-dropdown').filter({has:page.locator('summary',{hasText:'Abbigliamento · Collezioni'})});
  const resize = async size => {
    const wasDesktop=page.viewportSize().width>800;
    await page.setViewportSize(size);
    // Wait for the progressive-enhancement media listener before opening panels.
    if(js && wasDesktop!==(size.width>800)) {
      // Let the rendering cycle dispatch media events even when both old and new
      // navigation states are open; that listener also closes category panels.
      await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
      await page.waitForFunction(() => document.querySelector('.category-navigation').open===matchMedia('(min-width:801px)').matches);
    }
  };
  const open = async () => {
    if (!await menu.evaluate(e => e.open)) {await menu.locator(':scope > summary').focus();await page.keyboard.press('Enter');}
    if (!await dropdown.evaluate(e => e.open)) {await dropdown.locator(':scope > summary').focus();await page.keyboard.press('Enter');}
  };
  const settings = (await query('SELECT theme_id FROM storefront_settings WHERE id=1')).rows[0];
  try {
    const flatRoot = await create('Gadget compatti', 'mega-test-flat');
    for (let i=0;i<32;i++) await create(`Accessorio ${i+1}`,`mega-test-flat-${i}`,flatRoot,i);
    await page.goto(origin+'/');
    for (const theme of ['default','essential']) {
      await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[theme]);
      for (const prefix of ['', '/en']) {
        for (const width of [390,1440]) {
          await resize({width,height:1050});await page.goto(origin+prefix+'/');
          if (!await menu.evaluate(e=>e.open)) await menu.locator(':scope > summary').click();
          const flat=page.locator('.category-dropdown').filter({has:page.locator('summary',{hasText:'Gadget compatti'})});
          await flat.locator(':scope > summary').focus();await page.keyboard.press('Enter');
          const dimensions=await flat.evaluate(e=>{
            const grid=e.querySelector('.category-columns'),titles=[...grid.querySelectorAll('.category-column-title')];
            return {gap:parseFloat(getComputedStyle(grid).rowGap),items:titles.map(a=>{
              const s=getComputedStyle(a),r=a.getBoundingClientRect();
              return {font:parseFloat(s.fontSize),line:parseFloat(s.lineHeight),height:r.height,top:r.top};
            })};
          });
          assert.ok(dimensions.items.every(a=>a.font>=14 && a.font<=15 && a.line<=19),'Second-level category font and line height are compact');
          assert.ok(dimensions.gap<=16,'Compact row spacing for categories without descendants');
          assert.ok(dimensions.items.every(a=>Math.abs(a.height-(width<=800?44:36))<1),'Compact headings retain usable click targets');
          const columns=width<=500?1:4;
          assert.ok(dimensions.items[columns].top-dimensions.items[0].top<=60,'Second-level rows remain compact');
          assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          await flat.locator('.category-column-title').first().focus();
          assert.ok(await flat.locator('.category-column-title').first().evaluate(e=>e===document.activeElement && getComputedStyle(e).outlineStyle!=='none'));
          await page.screenshot({path:path.join(output,`mega-flat-${name}-${theme}-${prefix?'en':'it'}-${width}.png`)});
        }
      }
    }
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[settings.theme_id]);
    await resize(viewport);
    for (const [i,label] of ['Abbigliamento uomo','Abbigliamento donna','Abbigliamento bambino','Abbigliamento lavoro'].entries()) {
      const id = await create(label,`mega-test-branch-${i}`,root,i);
      branches.push(id);
      for (const [j,leaf] of ['T-shirt','Felpe','Giacche'].entries()) await create(leaf,`mega-test-leaf-${i}-${j}`,id,j);
    }
    const hidden = await create('Ramo nascosto','mega-test-hidden',root,4,false);
    await create('Figlia nascosta','mega-test-hidden-child',hidden);
    await page.goto(origin + '/');await open();
    assert.equal(await dropdown.locator('.category-column').count(),4);
    assert.equal(await dropdown.locator('.category-column-links').count(),4);
    assert.deepEqual(await dropdown.locator('.category-column-title').allTextContents(),['Abbigliamento uomo','Abbigliamento donna','Abbigliamento bambino','Abbigliamento lavoro']);
    for (let i=0;i<4;i++) assert.deepEqual(await dropdown.locator('.category-column').nth(i).locator('.category-column-links a').allTextContents(),['T-shirt','Felpe','Giacche']);
    assert.equal(await menu.getByText('Figlia nascosta',{exact:true}).count(),0);
    for (const width of [320,390,500,501,600,800,801,1099,1100,1280,1440,1920,2560]) {
      await resize({width,height:1050});await open();
      const geometry = await dropdown.evaluate(e => {
        const p=e.querySelector('.category-panel'),r=p.getBoundingClientRect();
        return {cols:getComputedStyle(e.querySelector('.category-columns')).gridTemplateColumns.split(' ').length,
          titles:[...e.querySelectorAll('.category-column-title')].map(a=>({x:a.getBoundingClientRect().x,y:a.getBoundingClientRect().y,font:parseFloat(getComputedStyle(a).fontSize)})),
          links:[...e.querySelector('.category-column-links').querySelectorAll('a')].map(a=>({top:a.getBoundingClientRect().top,height:a.getBoundingClientRect().height})),
          overflow:document.documentElement.scrollWidth>innerWidth,panelOverflow:p.scrollWidth>p.clientWidth,left:r.left,right:r.right,width:innerWidth};
      });
      assert.equal(geometry.cols,width<=500?1:width<=800?2:width<1100?3:4,JSON.stringify({width,geometry}));
      assert.equal(geometry.overflow,false);assert.equal(geometry.panelOverflow,false);
      assert.ok(geometry.left>=0 && geometry.right<=width+1);
      if(width>=1100) assert.ok(geometry.titles.every(t=>Math.abs(t.y-geometry.titles[0].y)<1),'All four branch headings align');
      if(width>800 && width<1100) {
        assert.ok(geometry.titles.slice(0,3).every(t=>Math.abs(t.y-geometry.titles[0].y)<1));
        assert.ok(geometry.titles[3].y>geometry.titles[0].y,'Fourth heading wraps at intermediate widths');
      }
      assert.ok(geometry.titles.every(t=>t.font>=14 && t.font<=15),'Compact headings remain readable');
      const step=width<=800?44:36;
      assert.ok(geometry.links.every(a=>Math.abs(a.height-step)<1),`Compact subcategory targets: ${JSON.stringify({width,step,links:geometry.links})}`);
      assert.ok(geometry.links.slice(1).every((a,i)=>Math.abs(a.top-geometry.links[i].top-step)<1),'Subcategory rows use compact vertical spacing');
      if([390,1440].includes(width)) await page.screenshot({path:path.join(output,`mega-menu-${name}-${width}.png`)});
    }
    await resize(viewport);await open();
    await dropdown.locator('.category-column-title').first().focus();await page.keyboard.press('Tab');
    const first = dropdown.locator('.category-column-links a').first();
    const focus=await first.evaluate(e=>({active:e===document.activeElement,outline:getComputedStyle(e).outlineStyle,
      target:document.activeElement.outerHTML,open:e.closest('details').open,navigation:document.querySelector('.category-navigation').open}));
    assert.ok(focus.active && focus.outline!=='none',JSON.stringify(focus));
    if(js) {
      await page.keyboard.press('Escape');assert.equal(await dropdown.evaluate(e=>e.open),false);
      assert.ok(await dropdown.locator(':scope > summary').evaluate(e=>e===document.activeElement));
      await page.keyboard.press('Enter');
      const brands=page.locator('.brand-dropdown');await brands.locator('summary').click();
      assert.equal(await dropdown.evaluate(e=>e.open),false);assert.equal(await brands.evaluate(e=>e.open),true);
      await page.getByLabel('Cerca un prodotto',{exact:true}).click();assert.equal(await brands.evaluate(e=>e.open),false);await open();
    }
    // Each level leads to its category and marks the owning macro category.
    for (const href of ['/catalog/mega-test-leaf-0-0','/catalog/mega-test-branch-1','/catalog/mega-test-root']) {
      await dropdown.locator(`a[href="${href}"]`).click();await page.waitForURL(origin+href);await open();
      assert.equal(await menu.locator('a[aria-current]').getAttribute('href'),href);
      assert.ok(await dropdown.locator('..').evaluate(e=>e.classList.contains('is-selected')));
    }
    // A populated column shows more entries before scrolling at a fixed height.
    for(let i=0;i<16;i++) await create(`Voce aggiuntiva ${i+1}`,`mega-test-dense-${i}`,branches[0],i+30);
    await resize({width:1440,height:844});await page.goto(origin+'/');await open();
    await dropdown.locator('.category-column-links a').first().focus();
    const visibleEntries=await dropdown.evaluate(e=>{
      const p=e.querySelector('.category-panel').getBoundingClientRect();
      return [...e.querySelector('.category-column-links').querySelectorAll('a')].filter(a=>{const r=a.getBoundingClientRect();return r.top>=p.top && r.bottom<=p.bottom;}).length;
    });
    assert.ok(visibleEntries>=8,`At least eight complete subcategories before scrolling: ${visibleEntries}`);
    await page.screenshot({path:path.join(output,`mega-menu-spacing-${name}.png`)});
    await resize(viewport);
    // More than four columns wrap; a second-level leaf remains directly usable.
    await create('Accessori per tutte le occasioni e le stagioni','mega-test-fifth',root,4);
    let parent=branches[0];
    for(let depth=3;depth<=16;depth++) parent=await create(`Categoria ${depth} con un nome molto lungo per controllare la leggibilità su ogni dispositivo`,'mega-test-deep-'+depth,parent,20);
    for(const width of [320,600,1440]) {
      await resize({width,height:844});await page.goto(origin+'/');await open();
      assert.equal(await dropdown.locator('.category-column').count(),5);
      assert.equal(await dropdown.locator('a[href="/catalog/mega-test-fifth"]').count(),1);
      const rows=await dropdown.locator('.category-column-title').evaluateAll(links=>links.map(a=>a.getBoundingClientRect().y));
      assert.ok(rows[4]>rows[0],'Fifth column wraps rather than widening the panel');
      const last=dropdown.locator('a[href="/catalog/mega-test-deep-16"]');await last.focus();
      assert.ok(await last.evaluate(e=>{const a=e.getBoundingClientRect(),p=e.closest('.category-panel').getBoundingClientRect();return e===document.activeElement && a.top>=p.top && a.bottom<=p.bottom+1;}),'Deep link scrolls into the bounded panel');
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    }
    await resize(viewport);
    await require('./browser-brand-logos.cjs')({page,query,origin,output,name,viewport});
  } finally {
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[settings.theme_id]);
    await query("DELETE FROM catalog_categories WHERE slug LIKE 'mega-test-%'");
    await page.goto(origin+'/');
  }
};
