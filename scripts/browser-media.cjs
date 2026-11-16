// Shared E2E exercised by every isolated demo profile, including mobile and no-JavaScript.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { createHash } = require('node:crypto');
module.exports = async ({page,query,origin,name,js,output,mediaRoot}) => {
  await require('./browser-media-sizing.cjs')({page,query,origin,name,output});
  await require('./browser-media-trash.cjs')({page,query,origin,name,output});
  await require('./browser-media-tree.cjs')({page,query,origin,name,js,output,mediaRoot});
  await require('./browser-media-maintenance-spacing.cjs')({page,query,origin,name,output});
  await require('./browser-media-picker.cjs')({page,query,origin,name,js,output});
  const folderName=`Caffè-${name}`;
  await page.goto(origin+'/admin/media');
  await page.locator('summary').filter({hasText:/^Nuova cartella$/}).click();
  await page.getByLabel('Nome della cartella',{exact:true}).fill(folderName);
  await page.getByRole('button',{name:'Crea cartella',exact:true}).click();
  await page.waitForURL(url=>url.pathname==='/admin/media'&&url.searchParams.has('folder'));
  const folder=Number(new URL(page.url()).searchParams.get('folder'));
  assert.ok(fs.statSync(path.join(mediaRoot,folderName)).isDirectory());
  assert.equal((await query('SELECT parent_id FROM media_folders WHERE id=$1',[folder])).rows[0].parent_id,null);
  await page.locator('summary').filter({hasText:/^Carica file$/}).click();
  await page.getByLabel('File da caricare',{exact:true}).setInputFiles([path.resolve('crates/web/tests/fixtures/avatar.png'),path.resolve('crates/web/tests/fixtures/webp-lossless.webp')]);
  const uploaded=page.waitForResponse(r=>new URL(r.url()).pathname==='/admin/media/upload'&&r.request().method()==='POST');
  await page.getByRole('button',{name:'Carica nella mediateca',exact:true}).click();assert.equal((await uploaded).status(),303);
  await page.waitForURL(url=>url.pathname==='/admin/media'&&url.searchParams.has('selected'));
  const assets=(await query('SELECT m.id,l.key,l.name,p.path FROM media_library l JOIN media_objects m ON m.key=l.key JOIN media_locations p ON p.key=l.key WHERE l.folder_id=$1 ORDER BY m.id',[folder])).rows;
  assert.equal(assets.length,2);assert.equal(await page.locator('.media-card').count(),2);
  const hashes=assets.map(a=>createHash('sha256').update(fs.readFileSync(path.join(mediaRoot,a.path))).digest('hex'));
  const product=(await query(`INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Media test','MEDIA-${name.toUpperCase()}','media-test-${name}',0) RETURNING id`)).rows[0].id;
  const picker=`/admin/media?target=gallery:${product}`;
  await page.goto(origin+picker);
  await page.locator('.media-tool summary').filter({hasText:/^Nuova cartella$/}).click();
  await page.locator('#folder-name').fill(`Product-${name}`);
  await page.getByRole('button',{name:'Crea cartella',exact:true}).click();
  await page.waitForURL(url=>url.searchParams.has('folder'));
  assert.equal(new URL(page.url()).searchParams.get('target'),`gallery:${product}`);
  assert.ok(fs.statSync(path.join(mediaRoot,`Product-${name}`)).isDirectory());
  await page.locator('.media-folder-root').click();
  await page.waitForURL(url=>!url.searchParams.has('folder'));
  if(js){
    for(const asset of assets)await page.locator(`[data-media-select="${asset.id}"]`).check();
    await page.getByRole('button',{name:'Aggiungi immagini selezionate',exact:true}).click();
    await page.waitForURL(url=>url.pathname===`/admin/catalog/products/${product}`);
  }else{
    for(const asset of assets){await page.goto(origin+picker);await page.locator(`[data-media-id="${asset.id}"]`).getByRole('button',{name:/^Seleziona /}).click();await page.waitForURL(url=>url.pathname===`/admin/catalog/products/${product}`);}
  }
  assert.equal(Number((await query('SELECT count(*) FROM catalog_product_images WHERE product_id=$1',[product])).rows[0].count),2);
  assert.equal(await page.locator('input[type=file]').count(),0);
  await page.goto(origin+`/admin/media/${assets[0].id}`);
  assert.equal(await page.getByRole('button',{name:'Sposta nel cestino',exact:true}).isEnabled(),false);
  await page.getByLabel('Nome del file',{exact:true}).fill('rinominata.png');
  await page.getByRole('button',{name:'Sposta / rinomina',exact:true}).click();
  await page.waitForURL(url=>url.pathname===`/admin/media/${assets[0].id}`&&url.searchParams.get('saved')==='true');
  const moved=(await query('SELECT l.key,l.name,p.path FROM media_library l JOIN media_objects m ON m.key=l.key JOIN media_locations p ON p.key=l.key WHERE m.id=$1',[assets[0].id])).rows[0];
  assert.equal(moved.key,assets[0].key);assert.equal(moved.name,'rinominata.png');assert.equal(createHash('sha256').update(fs.readFileSync(path.join(mediaRoot,moved.path))).digest('hex'),hashes[0]);
  const copied=page.waitForResponse(r=>new URL(r.url()).pathname===`/admin/media/${assets[0].id}`&&r.request().method()==='POST');
  await page.getByRole('button',{name:'Copia nella cartella',exact:true}).click();assert.equal((await copied).status(),303);
  await page.waitForURL(url=>url.searchParams.get('saved')==='true');
  assert.equal(Number((await query('SELECT count(*) FROM media_library WHERE folder_id=$1',[folder])).rows[0].count),3);
  await page.goto(origin+`/admin/media?folder=${folder}`);
  await page.locator('summary').filter({hasText:/^Gestisci cartella$/}).click();
  await page.getByLabel('Nome',{exact:true}).fill(`Archivio-${name}`);
  await page.getByRole('button',{name:'Sposta / rinomina cartella',exact:true}).click();await page.waitForURL(url=>url.pathname==='/admin/media'&&!url.searchParams.has('folder'));
  const changed=(await query('SELECT path FROM media_folders WHERE id=$1',[folder])).rows[0].path;assert.equal(changed,`Archivio-${name}`);
  for(const asset of assets){const located=(await query('SELECT path FROM media_locations WHERE key=$1',[asset.key])).rows[0].path;assert.ok(located.startsWith(changed+'/'));}
  const originalGallery=(await query('SELECT array_agg(image_key ORDER BY position) AS keys FROM catalog_product_images WHERE product_id=$1',[product])).rows[0].keys;assert.deepEqual(originalGallery,assets.map(a=>a.key));
  await query('DELETE FROM catalog_products WHERE id=$1',[product]);
  assert.equal(Number((await query('SELECT count(*) FROM media_library WHERE folder_id=$1',[folder])).rows[0].count),3,'Deleting a product retains library sources');
  await page.goto(origin+`/admin/media/${assets[1].id}`);
  await page.getByLabel('Confermo lo spostamento nel cestino',{exact:true}).check();await page.getByRole('button',{name:'Sposta nel cestino',exact:true}).click();await page.getByRole('button',{name:'Ripristina immagine',exact:true}).waitFor();
  await page.goto(origin+'/admin/media?trash=true');assert.equal(await page.locator(`[data-media-id="${assets[1].id}"]`).count(),1);
  await page.goto(origin+`/admin/media/${assets[1].id}`);await page.getByRole('button',{name:'Ripristina immagine',exact:true}).click();await page.getByRole('button',{name:'Sposta nel cestino',exact:true}).waitFor();
  await page.goto(origin+`/admin/media?folder=${folder}`);await page.getByLabel('Cerca file',{exact:true}).fill('rinominata');await page.getByRole('button',{name:'Cerca',exact:true}).click();await page.waitForURL(url=>url.searchParams.get('q')==='rinominata');assert.equal(await page.locator('.media-card').count(),2);
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.getByLabel('Cerca file',{exact:true}).focus();assert.equal(await page.evaluate(()=>document.activeElement.id),'media-search');
  await page.screenshot({path:path.join(output,`media-library-${name}.png`),fullPage:true});
  await page.goto(origin+`/admin/media/${assets[0].id}`);await page.screenshot({path:path.join(output,`media-detail-${name}.png`),fullPage:true});
  const download=await page.context().request.get(origin+`/admin/media/${assets[0].id}/download`);assert.equal(download.status(),200);assert.equal(createHash('sha256').update(await download.body()).digest('hex'),hashes[0]);
  // Remove only an isolated test cache revision; real originals and application media are untouched.
  assert.ok(mediaRoot.includes('talocart-demo-browser-'));
  const cachePath=async()=>((await query("SELECT p.path FROM media_renditions r JOIN media_locations p ON p.key=r.object_key JOIN media_cache_jobs j ON j.source_key=r.source_key WHERE r.source_key=$1 AND r.profile='card' AND j.status='complete'",[assets[0].key])).rows[0]?.path);
  let cached;
  for(let n=0;n<100&&!cached;n++){cached=await cachePath();if(!cached)await new Promise(r=>setTimeout(r,100));}
  assert.ok(cached?.startsWith('cache/'));
  fs.rmSync(path.dirname(path.join(mediaRoot,cached)),{recursive:true});
  const fallback=await page.context().request.get(origin+`/media/image/${assets[0].id}/card`);
  assert.equal(fallback.status(),200);assert.match(fallback.headers()['content-type'],/^image\/(png|jpeg|webp)$/);
  let repaired;
  for(let n=0;n<100;n++){const current=await cachePath();if(current&&current!==cached){repaired=current;break;}await new Promise(r=>setTimeout(r,100));}
  assert.ok(repaired,'The real worker automatically repairs a deleted cache revision');
  assert.ok(fs.statSync(path.join(mediaRoot,repaired)).isFile());
  const currentURL=origin+`/media/image/${assets[0].id}/card`;
  const current=await page.context().request.get(currentURL);assert.equal(current.status(),200);
  assert.equal(current.url(),currentURL,'Profile images serve the repaired representation directly');
  assert.equal(createHash('sha256').update(await current.body()).digest('hex'),
    createHash('sha256').update(fs.readFileSync(path.join(mediaRoot,repaired))).digest('hex'),
    'The response contains the repaired cache object');
  const source=(await query('SELECT path FROM media_locations WHERE key=$1',[assets[0].key])).rows[0].path;
  assert.equal(createHash('sha256').update(fs.readFileSync(path.join(mediaRoot,source))).digest('hex'),hashes[0]);
  // The normal maintenance command leaves valid cache versions intact, with native form fallback.
  await page.goto(origin+'/admin/media');
  await page.locator('summary').filter({hasText:/^Manutenzione immagini$/}).click();
  const maintenance=page.locator('form[action="/admin/media/cache"]');
  await maintenance.getByLabel('Confermo l’operazione sulla cache immagini',{exact:true}).check();
  await maintenance.getByRole('button',{name:'Ripara immagini',exact:true}).focus();
  const repairedResponse=page.waitForResponse(r=>new URL(r.url()).pathname==='/admin/media/cache'&&r.request().method()==='POST');
  await page.keyboard.press('Enter'); assert.equal((await repairedResponse).status(),303);
  await page.waitForURL(url=>url.pathname==='/admin/media'&&url.searchParams.get('saved')==='true');
  let settled=false;
  for(let n=0;n<200;n++) {if(await cachePath()){settled=true;break;}await new Promise(r=>setTimeout(r,100));}
  assert.ok(settled);assert.equal(await cachePath(),repaired,'Repair preserves healthy versioned URLs');
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.locator('summary').filter({hasText:/^Manutenzione immagini$/}).click();
  await page.screenshot({path:path.join(output,`media-maintenance-${name}.png`),fullPage:true});
  await page.goto(origin+'/admin');
};
