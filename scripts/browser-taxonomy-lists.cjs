// Shared directory layout: synthetic rows only, inside the demo suite's isolated schema.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, query, origin, name, output}) => {
  const viewport=page.viewportSize();
  const prefix='taxonomy-layout-';
  const longName='Categoria e marchio con un nome molto lungo & testo da conservare '.repeat(2).slice(0,120);
  const parent=(await query("INSERT INTO catalog_categories(name,slug) VALUES($1,$2) RETURNING id",[longName,prefix+'parent'])).rows[0].id;
  const logo=(await query('SELECT logo_key FROM catalog_brands WHERE logo_key IS NOT NULL LIMIT 1')).rows[0]?.logo_key;
  try {
    for(const table of ['catalog_categories','catalog_brands']) {
      for(let n=0;n<23;n++) {
        const slug=prefix+String(n).padStart(2,'0')+'-'+('a'.repeat(130));
        const columns=table==='catalog_categories'?'parent_id':'logo_key';
        await query(`INSERT INTO ${table}(name,slug,sort_order,active,${columns}) VALUES($1,$2,$3,$4,$5)`,[n===0?longName:`Voce grafica ${String(n).padStart(2,'0')}`,slug,n,n%2===0,table==='catalog_categories'?parent:n===0?logo:null]);
      }
    }
    for(const kind of ['brands']) {
      const base=`/admin/catalog/${kind}`;
      await page.setViewportSize({width:1920,height:1080});
      await page.goto(origin+base+'?q='+prefix);
      // Capture the real sequence and geometry before asserting the new layout.
      await page.screenshot({path:path.join(output,`taxonomy-${kind}-${name}-1920.png`),fullPage:true});
      const geometry=await page.locator('.catalog-list [role=listitem]').evaluateAll(items=>items.map(item=>{const r=item.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width};}));
      const firstLine=geometry.filter(r=>Math.abs(r.y-geometry[0].y)<2);
      assert.ok(firstLine.length>=3,`${kind}: several entries share a desktop row instead of scattering their fields across the screen`);
      assert.ok(firstLine.every(r=>r.width<=440),`${kind}: readable bounded card width on large screens`);
      assert.equal(await page.locator('.taxonomy-card').count(),20);
      for(const width of [320,390,768,1280,1440,1920,2560]) {
        await page.setViewportSize({width,height:1080});
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${kind}: no overflow at ${width}`);
        const boxes=await page.locator('.taxonomy-card').evaluateAll(cards=>cards.map(card=>{
          const r=card.getBoundingClientRect();const action=card.querySelector('.catalog-edit').getBoundingClientRect();
          return {x:r.x,right:r.right,y:r.y,width:r.width,scroll:card.scrollWidth,client:card.clientWidth,actionX:action.x,actionRight:action.right,actionHeight:action.height};
        }));
        assert.ok(boxes.every(b=>b.scroll<=b.client&&b.actionX>b.x&&b.actionRight<b.right&&b.actionHeight===36),`${kind}: card contents and compact actions stay inside at ${width}`);
        const line=boxes.filter(b=>Math.abs(b.y-boxes[0].y)<2);
        if(width<=390)assert.equal(line.length,1,`${kind}: one card per mobile row`);
        if(width>=1440)assert.ok(line.length>=3,`${kind}: desktop grid uses the central space`);
        const edit=page.locator('.taxonomy-card .catalog-edit').first();await edit.focus();
        assert.ok(await edit.evaluate(e=>document.activeElement===e&&getComputedStyle(e).outlineStyle==='solid'),`${kind}: visible keyboard focus`);
        if([390,1440,2560].includes(width))await page.screenshot({path:path.join(output,`taxonomy-${kind}-${name}-${width}.png`),fullPage:true});
      }
      await page.getByRole('navigation',{name:`Pagine ${kind==='brands'?'Marchi':'Categorie'}`}).getByRole('link',{name:'Successivo',exact:true}).click();
      await page.waitForURL(u=>u.searchParams.get('page')==='2');assert.ok((await page.locator('.taxonomy-card').count())<20);assert.equal(await page.getByLabel('Cerca per nome o slug').inputValue(),prefix);
      await page.getByLabel('Stato',{exact:true}).selectOption('inactive');await page.getByRole('button',{name:'Filtra',exact:true}).click();await page.waitForURL(u=>u.searchParams.get('status')==='inactive'&&!u.searchParams.has('page'));
      assert.equal(await page.locator('.taxonomy-card').count(),11);assert.equal(await page.locator('.taxonomy-card .catalog-status.inactive').count(),11);
      await page.getByLabel('Cerca per nome o slug').fill('taxonomy-layout-absent');await page.getByRole('button',{name:'Filtra',exact:true}).click();await page.waitForURL(u=>u.searchParams.get('q')==='taxonomy-layout-absent');assert.equal(await page.locator('.ui-empty').count(),1);assert.equal(await page.locator('.taxonomy-card').count(),0);
      await page.getByRole('link',{name:'Azzera filtri',exact:true}).click();await page.waitForURL('**'+base);
      const edit=page.locator('.taxonomy-card .catalog-edit').first();const href=await edit.getAttribute('href');await edit.focus();await page.keyboard.press('Enter');await page.waitForURL('**'+href);await page.getByLabel(kind==='brands'?'Nome del marchio':'Nome della categoria',{exact:true}).waitFor();
      // A second view uses short, realistic synthetic labels for visual review.
      const table=kind==='brands'?'catalog_brands':'catalog_categories';
      const labels=kind==='brands'?['Officina Tessile','Linea Verde','Atelier Koala','Forma Studio','Trama & Colore','Essenziale']:['Abbigliamento','Accessori','Casa e cucina','Sport e tempo libero','Ufficio','Viaggio'];
      const ids=(await query(`SELECT id FROM ${table} WHERE slug LIKE 'taxonomy-layout-%' ORDER BY id`)).rows;
      for(let n=0;n<ids.length;n++)await query(`UPDATE ${table} SET name=$1,slug=$2 WHERE id=$3`,[`${labels[n%labels.length]} ${String(n+1).padStart(2,'0')}`,prefix+String(n+1).padStart(2,'0'),ids[n].id]);
      for(const width of [390,1440,2560]) {
        await page.setViewportSize({width,height:1000});await page.goto(origin+base+'?q='+prefix);
        if(width===390)await page.locator('.taxonomy-card').first().scrollIntoViewIfNeeded();
        await page.screenshot({path:path.join(output,`taxonomy-${kind}-preview-${name}-${width}.png`)});
      }

    }
  } finally {
    await page.setViewportSize(viewport);
    await query("DELETE FROM catalog_categories WHERE slug LIKE 'taxonomy-layout-%'");
    await query("DELETE FROM catalog_brands WHERE slug LIKE 'taxonomy-layout-%'");
  }
};
