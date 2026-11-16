// Tree contract and visuals against synthetic nodes in the demo suite's isolated schema.
const assert=require('node:assert/strict');const path=require('node:path');
module.exports=async({page,query,origin,name,output})=>{
  const viewport=page.viewportSize();const prefix='tree-layout-';let root,child,grandchild;
  const add=async(label,slug,parent=null,order=0,active=true)=>(await query('INSERT INTO catalog_categories(name,slug,parent_id,sort_order,active) VALUES($1,$2,$3,$4,$5) RETURNING id',[label,prefix+slug,parent,order,active])).rows[0].id;
  try{
    root=await add('Abbigliamento & accessori','root',null,2,false);
    child=await add('Uomo','child',root,1);grandchild=await add('Polo e camicie','grandchild',child,1);
    await add('Donna','sibling',root,2);await add('Casa','other',null,1);
    for(let n=0;n<23;n++)await add('Accessori '+String(n).padStart(2,'0'),'page-'+n,root,10+n,n%2===0);
    let parent=grandchild;
    for(let n=3;n<16;n++)parent=await add(n===15?'Nome molto lungo & sottocategoria '.repeat(4).slice(0,120):'Livello '+(n+1),'deep-'+n,parent);
    const base='/admin/catalog/categories';
    await page.setViewportSize({width:1440,height:1000});await page.goto(origin+base+'?q='+prefix);
    await page.screenshot({path:path.join(output,`category-tree-before-or-after-${name}.png`)});
    const nesting=await page.locator(`[data-category-id="${grandchild}"]`).evaluate(e=>e.parentElement.closest('[data-category-id]').getAttribute('data-category-id'));
    assert.equal(nesting,String(child),'Grandchild is structurally nested under its parent');
    assert.equal(await page.locator(`[data-category-id="${child}"]`).evaluate(e=>e.parentElement.closest('[data-category-id]').getAttribute('data-category-id')),String(root));
    for(const width of [320,390,768,1280,1440,1920,2560]){
      await page.setViewportSize({width,height:1000});
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`Tree has no overflow at ${width}`);
      const nodes=await page.locator('.category-tree-directory .taxonomy-card').evaluateAll(cards=>cards.map(c=>{const r=c.getBoundingClientRect();const a=c.querySelector('.catalog-edit').getBoundingClientRect();return {scroll:c.scrollWidth,client:c.clientWidth,left:r.left,right:r.right,actionLeft:a.left,actionRight:a.right};}));
      assert.ok(nodes.every(c=>c.scroll<=c.client&&c.actionLeft>c.left&&c.actionRight<c.right),`Tree contents fit at ${width}, including 16 levels`);
      const offset=await page.locator(`[data-category-id="${grandchild}"] > article`).evaluate((c,root)=>c.getBoundingClientRect().left-document.querySelector(`[data-category-id="${root}"] > article`).getBoundingClientRect().left,root);
      assert.ok(offset>0,'Descendants are visibly indented');
      const edit=page.locator(`[data-category-id="${child}"] > article .catalog-edit`);await edit.focus();assert.ok(await edit.evaluate(e=>document.activeElement===e&&getComputedStyle(e).outlineStyle==='solid'));
      if([390,1440,2560].includes(width)){await page.locator('.category-tree-directory').scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,`category-tree-${name}-${width}.png`)});}
    }
    await page.getByRole('navigation',{name:'Pagine Categorie'}).getByRole('link',{name:'Successivo',exact:true}).click();await page.waitForURL(u=>u.searchParams.get('page')==='2');
    assert.ok(await page.locator(`[data-category-id="${root}"]`).count());assert.equal(await page.locator(`[data-category-id="${root}"]`).getAttribute('data-context'),'true');
    await page.getByLabel('Cerca per nome o slug').fill(prefix+'grandchild');await page.getByLabel('Stato',{exact:true}).selectOption('active');await page.getByRole('button',{name:'Filtra',exact:true}).click();await page.waitForURL(u=>u.searchParams.get('q')===prefix+'grandchild');
    assert.equal(await page.locator('.category-tree-directory [data-context=false]').count(),1);assert.equal(await page.locator('.category-tree-directory [data-context=true]').count(),2);
    assert.equal(await page.locator('.taxonomy-total').innerText(),'1 categoria');assert.equal(await page.locator(`[data-category-id="${root}"] > article .catalog-status.inactive`).count(),1);
    await page.locator(`[data-category-id="${grandchild}"] > article .catalog-edit`).focus();await page.keyboard.press('Enter');await page.waitForURL('**/admin/catalog/categories/'+grandchild);await page.getByLabel('Nome della categoria',{exact:true}).waitFor();
    await page.goto(origin+base+'?q='+prefix+'missing');assert.equal(await page.locator('.ui-empty').count(),1);assert.equal(await page.locator('.category-tree-directory').count(),0);
    await page.getByRole('link',{name:'Azzera filtri',exact:true}).click();await page.waitForURL('**'+base);
    // Review the familiar cards on a short, realistic three-level tree as well.
    await query("DELETE FROM catalog_categories WHERE slug LIKE 'tree-layout-deep-%'");
    for(const width of [390,1440,2560]){
      await page.setViewportSize({width,height:1100});await page.goto(origin+base+'?q='+prefix);
      await page.locator(`[data-category-id="${grandchild}"] > article`).scrollIntoViewIfNeeded();
      await page.screenshot({path:path.join(output,`category-tree-preview-${name}-${width}.png`)});
    }

  }finally{await page.setViewportSize(viewport);await query("DELETE FROM catalog_categories WHERE slug LIKE 'tree-layout-%'");}
};
