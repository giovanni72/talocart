// Called inside each isolated demo profile. Synthetic rows are removed in finally.
const assert = require('node:assert/strict');
const path = require('node:path');
const fs = require('node:fs');
module.exports = async ({page,query,origin,name,output,mediaRoot,importsEnabled=false}) => {
  const prefix='Pagination extension ';
  const original=page.viewportSize();
  try {
    // The folder tree only renders pagination when a branch has more than 100 children.
    await query("INSERT INTO media_folders(name,path) SELECT 'Pagination extension folder '||lpad(n::text,3,'0'),'pagination-extension-folder-'||n FROM generate_series(1,103)n");
    for(let n=1;n<=103;n++) fs.mkdirSync(path.join(mediaRoot,`pagination-extension-folder-${n}`),{recursive:true});
    await query("INSERT INTO catalog_categories(name,slug) SELECT 'Pagination extension '||n,'pagination-extension-'||n FROM generate_series(1,23)n");
    await query("INSERT INTO catalog_brands(name,slug) SELECT 'Pagination extension '||n,'pagination-extension-'||n FROM generate_series(1,103)n");
    await query("INSERT INTO tax_rates(name,basis_points) SELECT 'Pagination extension '||n,2200 FROM generate_series(1,23)n");
    await query("INSERT INTO tax_classes(name,rate_id) SELECT 'Pagination extension '||n,(SELECT min(id) FROM tax_rates) FROM generate_series(1,23)n");
    await query("INSERT INTO admin_roles(key,name) SELECT 'role_'||lpad(to_hex(10000+n),16,'0'),'Pagination extension '||n FROM generate_series(1,23)n");
    await query("INSERT INTO administrators(email,password_hash,role) SELECT 'pagination-extension-'||n||'@example.test',a.password_hash,'viewer' FROM administrators a CROSS JOIN generate_series(1,23)n WHERE a.role='owner' ORDER BY a.id LIMIT 23");
    const option=(await query("INSERT INTO catalog_options(name) VALUES('Pagination extension values') RETURNING id")).rows[0].id;
    await query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Paging value '||n FROM generate_series(1,53)n",[option]);
    const cases=[
      ['/admin/catalog/categories?q=Pagination%20extension&status=active','Pagine Categorie','Categorie','categories'],
      ['/admin/catalog/brands?q=Pagination%20extension&status=active','Pagine Marchi','Marchi','brands-admin'],
      ['/admin/settings/taxes/rates?q=Pagination%20extension&status=active','Pagine Aliquote','Aliquote','rates'],
      ['/admin/settings/taxes/classes?q=Pagination%20extension&status=active','Pagine Classi fiscali','Classi fiscali','classes'],
      ['/admin/catalog/options','Pagine opzioni','Opzioni','options'],
      [`/admin/catalog/options/${option}`,'Pagine valori','Valori','values'],
      ['/admin/users','Pagine amministratori','Amministratori','users'],
      ['/admin/roles','Pagine ruoli','Ruoli','roles'],
      ['/brands','Pagine marchi','Marchi','brands-public'],
      ['/admin/media','Paginazione file','File','media'],
      ['/admin/media','Pagine cartelle','Cartelle','folders'],
      ['/admin/extensions/koala-import/imports','Pagine importazioni','Importazioni','imports'],
    ];
    const product=(await query("SELECT product_id AS id FROM catalog_variants GROUP BY product_id ORDER BY count(*) DESC LIMIT 1")).rows[0];
    if(product) cases.push([`/admin/catalog/products/${product.id}/variants`,'Pagine varianti','Varianti','variants']);
    const asset=(await query('SELECT m.id FROM media_library l JOIN media_objects m ON m.key=l.key WHERE l.owner_id IS NULL ORDER BY m.id LIMIT 1')).rows[0];
    if(asset) cases.push([`/admin/media/${asset.id}`,'Pagine utilizzi','Utilizzi','usages']);
    for(const [route,aria,label,key] of cases) {
      if(key==='imports' && !importsEnabled) {
        assert.equal(await page.locator('.admin-sidebar').count(),1);
        assert.equal(await page.locator('.admin-sidebar a[href="/admin/extensions/koala-import/imports"]').count(),0);
        const response=await page.goto(origin+route);
        assert.equal(response.status(),404,'The base does not expose disabled external imports');
        continue;
      }
      await checkCase({page,origin,name,output},[route,aria,label,key]);
    }
  } finally {
    for(let n=1;n<=103;n++) fs.rmSync(path.join(mediaRoot,`pagination-extension-folder-${n}`),{recursive:true,force:true});
    await query("DELETE FROM media_folders WHERE path LIKE 'pagination-extension-folder-%'");
    await page.setViewportSize(original);
    await query("DELETE FROM catalog_option_values WHERE option_id IN(SELECT id FROM catalog_options WHERE name LIKE $1)",[prefix+'%']);
    await query('DELETE FROM catalog_options WHERE name LIKE $1',[prefix+'%']);
    await query("DELETE FROM administrators WHERE email LIKE 'pagination-extension-%@example.test'");
    await query('DELETE FROM admin_roles WHERE name LIKE $1',[prefix+'%']);
    await query('DELETE FROM tax_classes WHERE name LIKE $1',[prefix+'%']);
    await query('DELETE FROM tax_rates WHERE name LIKE $1',[prefix+'%']);
    await query('DELETE FROM catalog_brands WHERE name LIKE $1',[prefix+'%']);
    await query('DELETE FROM catalog_categories WHERE name LIKE $1',[prefix+'%']);
  }
};

async function checkCase({page,origin,name,output},[route,aria,label,key]) {
  const response=await page.goto(origin+route);
  assert.equal(response.status(),200,key);
  const nav=page.getByRole('navigation',{name:aria,exact:true});
  assert.equal(await nav.count(),1,key);
  assert.match(await nav.locator('.pagination-summary').innerText(),new RegExp(`^${label} [0-9–]+ di [0-9]+\\nPagina [0-9]+ di [0-9]+$`));
  assert.equal(await nav.locator('.pagination-button').count(),2);
  for(const width of [320,1440]) {
    await page.setViewportSize({width,height:1000});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${key} overflow at ${width}`);
    for(const button of await nav.locator('.pagination-button').all()) {
      const box=await button.boundingBox();
      assert.ok(box.height>=44 && box.x>=0 && box.x+box.width<=width,`${key} button geometry`);
    }
    await nav.screenshot({path:path.join(output,`extended-pagination-${key}-${name}-${width}.png`)});
  }
  const next=nav.getByRole('link',{name:'Successivo',exact:true});
  if(await next.count()) {
    await next.focus();
    assert.ok(await next.evaluate(e=>e===document.activeElement&&getComputedStyle(e).outlineStyle!=='none'));
    await page.keyboard.press('Enter');
    await page.waitForURL(u=>u.searchParams.get('page')==='2' || u.searchParams.get('folders_page')==='2' || u.searchParams.get('uses_page')==='2');
    assert.ok((await nav.locator('.pagination-summary').innerText()).includes('Pagina 2 di '));
    if(route.includes('q=')) assert.equal(new URL(page.url()).searchParams.get('q'),'Pagination extension');
    await nav.getByRole('link',{name:'Precedente',exact:true}).click();
    assert.ok((await nav.locator('.pagination-summary').innerText()).includes('Pagina 1 di '));
  }
}

// The enabled-module fixture exercises the same pagination assertions; no check is dropped.
module.exports.imports = async args => {
  const original=args.page.viewportSize();
  try { await checkCase(args,['/admin/extensions/koala-import/imports','Pagine importazioni','Importazioni','imports']); }
  finally { await args.page.setViewportSize(original); }
};
