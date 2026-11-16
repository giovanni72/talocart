// Shared native picker: folder browsing, upload, explicit selection and preserved drafts.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, query, origin, name, js, output}) => {
  const original = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  const theme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  const whiteText = async button => {
    const colors = await button.evaluate(e=>{const s=getComputedStyle(e);return {foreground:s.color.match(/\d+/g).slice(0,3).map(Number),background:s.backgroundColor.match(/\d+/g).slice(0,3).map(Number)};});
    assert.ok(colors.foreground.every(c=>c>=245),'White selection text: '+JSON.stringify(colors));
    const luminance = rgb=>rgb.map(c=>c/255).map(c=>c<=.04045?c/12.92:((c+.055)/1.055)**2.4).reduce((sum,c,i)=>sum+c*[.2126,.7152,.0722][i],0);
    assert.ok((luminance(colors.foreground)+.05)/(luminance(colors.background)+.05)>=4.5,'Selection text contrast must reach 4.5:1');
  };
  const submit = async (action) => {
    const button=page.locator(`button[name=action][value="${action}"]:visible`).first();
    const done = page.waitForResponse(r => r.request().method() === 'POST');
    await button.click();
    assert.equal((await done).status(), 200, action); await page.waitForLoadState('load');
  };
  try {
    await page.goto(origin+'/admin/media');
    const csrf = await page.locator('[name=csrf]').first().inputValue();
    const folderName = 'Picker '+name;
    const created = await page.request.post(origin+'/admin/media/folders', {form:{csrf,name:folderName},maxRedirects:0});
    assert.equal(created.status(),303);
    const folder = Number(new URL(created.headers().location,origin).searchParams.get('folder'));
    const sub = await page.request.post(origin+'/admin/media/folders',{form:{csrf,folder:String(folder),name:'Nested'},maxRedirects:0});assert.equal(sub.status(),303);
    const nested = Number(new URL(sub.headers().location,origin).searchParams.get('folder'));
    // The compact Select regression is checked before testing the new draft picker.
    await page.goto(origin+'/admin/media?target=logo');
    const select = page.locator('.media-card button[name=ids]').first();
    assert.equal((await select.innerText()).trim(),original==='it'?'Seleziona':'Select','Visible Select label must not repeat the filename');
    await whiteText(select);
    const firstCard=page.locator('.media-card').first(), initialId=await firstCard.getAttribute('data-media-id');
    const initialName=await firstCard.locator('strong').innerText();
    const product=(await query('SELECT id FROM catalog_products ORDER BY id LIMIT 1')).rows[0].id;
    const variant=(await query('SELECT id,product_id FROM catalog_variants ORDER BY id LIMIT 1')).rows[0];
    for(const target of [`product:${product}`,`gallery:${product}`,`variant:${variant.product_id}:${variant.id}`]) {
      await page.goto(origin+`/admin/media?target=${encodeURIComponent(target)}&q=${encodeURIComponent(initialName)}&sort=newest&page=1&folders_page=1`);
      await page.locator(`[data-media-id="${initialId}"] .media-preview`).click();await page.waitForLoadState('load');
      assert.equal(new URL(page.url()).searchParams.get('target'),target);
      assert.equal(await page.locator('form[action="/admin/media/select"] [name=target]').inputValue(),target);
      await page.locator('main .ui-page-header__back').click();await page.waitForLoadState('load');
      assert.equal(new URL(page.url()).searchParams.get('target'),target);
      assert.equal(await page.locator('#media-search').inputValue(),initialName);
      assert.equal(await page.locator('#media-sort').inputValue(),'newest');
      assert.equal(await page.locator('form[action="/admin/media/select"] [name=target]').first().inputValue(),target);
    }
    await page.goto(origin+'/admin/media?target=logo');
    const folderStyle=await page.locator('.media-folder-root').evaluate(e=>{const s=getComputedStyle(e);return [s.padding,s.borderRadius,s.minHeight,s.backgroundColor,s.boxShadow];});
    for (const currentTheme of ['default','essential']) for (const locale of ['en','it']) {
      await query('UPDATE administrators SET locale=$1',[locale]); await query('UPDATE storefront_settings SET theme_id=$1',[currentTheme]);
      for (const editor of ['banner','settings']) {
        await page.goto(origin+(editor==='banner'?'/admin/banners/new':'/admin/settings/appearance'));
        const draft = `Draft ${name} ${currentTheme} ${locale} ${editor}`;
        const draftInput = page.locator(editor==='banner'?'#banner-label':'#store-name');
        const preserved = () => page.locator(`#media-draft [name="${editor==='banner'?'label':'name'}"]`).inputValue();
        if(editor==='settings') await page.locator('#settings-tab-general').click();
        await draftInput.fill(draft);
        if(editor==='settings') { await page.locator('#settings-tab-countries').click();await page.locator('#checkout-countries').selectOption(['IT','FR']);await page.locator('#settings-tab-appearance').click(); }
        else { await page.locator('#banner-name').fill('Alt <&> '+draft);await page.locator('#banner-content-en-name').fill('English '+draft); }
        await submit(editor==='banner'?'browse':'browse_logo');
        assert.equal(await page.locator('main.media-main').count(),1,'Every image selection must use the canonical media page');
        assert.ok(!/banners-|settings-/.test(await page.locator('h1').innerText()),'The media page title is localized');
        assert.equal(await page.locator('.media-upload[open] input[type=file]').isVisible(),true,'Upload is immediately visible');
        assert.equal(await page.locator('[value=media_upload]').getAttribute('formenctype'),'multipart/form-data');
        assert.deepEqual(await page.locator('.media-folder-root').evaluate(e=>{const s=getComputedStyle(e);return [s.padding,s.borderRadius,s.minHeight,s.backgroundColor,s.boxShadow];}),folderStyle,'Active folder matches the canonical library component');
        // Hover must not collapse padding, height or rounded corners in native folder buttons.
        const folderButton=page.locator('.media-folder-root');
        const folderGeometry=()=>folderButton.evaluate(e=>{const s=getComputedStyle(e),r=e.getBoundingClientRect();return [r.width,r.height,s.padding,s.borderRadius];});
        await page.mouse.move(0,0);const beforeHover=await folderGeometry();
        await folderButton.hover();assert.deepEqual(await folderGeometry(),beforeHover,'Folder geometry must remain stable on hover');
        await page.mouse.move(0,0);
        const geometry=await page.locator('.media-filters').evaluate(e=>{const input=e.querySelector('input[type=search]').getBoundingClientRect(),button=e.querySelector('button').getBoundingClientRect();return {input:{bottom:input.bottom,top:input.top},button:{bottom:button.bottom,top:button.top}};});
        if(geometry.button.top<geometry.input.bottom) assert.ok(Math.abs(geometry.input.bottom-geometry.button.bottom)<1,'Search button aligns with its input');
        assert.equal(await page.locator('#media-format').count(),1);assert.equal(await page.locator('#media-usage').count(),1);assert.equal(await page.locator('#media-sort').count(),1);
        if(editor==='settings') assert.deepEqual(await page.locator('#media-draft [name=purchase_country]').evaluateAll(nodes=>nodes.map(n=>n.value).sort()),['FR','IT']);
        else assert.equal(await page.locator('#media-draft [name=name]').inputValue(),'Alt <&> '+draft);
        const newFolder = `Picker-${name}-${currentTheme}-${locale}-${editor}`;
        await page.locator('.media-tool summary').filter({hasText:locale==='it'?'Nuova cartella':'New folder'}).click();
        await page.locator('#folder-name').fill(newFolder);
        const folderCreated=page.waitForResponse(r=>r.request().method()==='POST');
        await page.locator('#folder-name').press('Enter');
        assert.equal((await folderCreated).status(),200);await page.waitForLoadState('load');
        const created=(await query('SELECT id,parent_id,path FROM media_folders WHERE name=$1',[newFolder])).rows[0];
        assert.ok(created);assert.equal(created.parent_id,null);assert.equal(created.path,newFolder);
        assert.equal(await preserved(),draft);
        assert.equal(await page.locator('#media-draft [name=media_folder]').inputValue(),String(created.id));
        await page.locator('.media-tool summary').filter({hasText:locale==='it'?'Nuova cartella':'New folder'}).click();
        await page.locator('#folder-name').fill('Nested');
        await submit('media_create_folder');
        assert.equal((await query('SELECT parent_id FROM media_folders WHERE path=$1',[`${newFolder}/Nested`])).rows[0].parent_id,created.id);
        assert.equal(await preserved(),draft);
        await submit('media_folder_'+created.id);
        await page.locator('.media-tool summary').filter({hasText:locale==='it'?'Nuova cartella':'New folder'}).click();
        await page.locator('#folder-name').fill('Nested');
        const rejected=page.waitForResponse(r=>r.request().method()==='POST');
        await page.locator('[value=media_create_folder]').focus();await page.keyboard.press('Enter');
        assert.equal((await rejected).status(),409);
        await page.locator('#media-error').waitFor({state:'visible'});
        assert.equal(await page.locator('#media-error').count(),1);
        assert.equal(await page.locator('#folder-name').getAttribute('aria-invalid'),'true');
        assert.equal(await page.locator('[name=picker_image]').getAttribute('aria-invalid'),null);
        assert.equal(await page.locator('#folder-name').inputValue(),'Nested');
        assert.equal(await preserved(),draft);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
        await page.screenshot({path:path.join(output,`media-picker-folder-${name}-${currentTheme}-${locale}-${editor}.png`),fullPage:true});
        await submit('media_folder_0');
        const chain=(await query('WITH RECURSIVE chain AS (SELECT id,parent_id,0 depth FROM media_folders WHERE id=$1 UNION ALL SELECT f.id,f.parent_id,c.depth+1 FROM media_folders f JOIN chain c ON f.id=c.parent_id) SELECT id FROM chain ORDER BY depth DESC',[nested])).rows;
        if(js && chain.length>1) {
          const currentUrl=page.url(), first=chain[0].id;
          const loaded=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname==='/admin/media/tree/'+first);
          await page.locator(`details[data-folder="${first}"] > summary`).focus();await page.keyboard.press('Enter');assert.equal((await loaded).status(),200);
          await page.locator(`[data-folder-link="${chain[1].id}"]`).waitFor();assert.equal(page.url(),currentUrl);assert.equal(await preserved(),draft);
        }
        for(const node of chain) await submit('media_folder_'+node.id);
        assert.equal(await preserved(),draft);
        assert.equal(await page.locator('#media-draft [name=media_folder]').inputValue(),String(nested));
        assert.equal(await page.locator('.media-card').count(),(await query('SELECT count(*)::int n FROM media_library WHERE folder_id=$1 AND trashed_at IS NULL',[nested])).rows[0].n);
        await page.locator('[name=picker_image]').setInputFiles({name:'invalid.png',mimeType:'image/png',buffer:Buffer.from('invalid image')});
        await submit('media_upload'); assert.equal(await page.locator('#media-error').count(),1);assert.equal(await preserved(),draft);
        await page.locator('[name=picker_image]').setInputFiles(Array.from({length:11},(_,i)=>({name:`overflow-${i}.png`,mimeType:'image/png',buffer:require('node:fs').readFileSync('crates/web/tests/fixtures/avatar.png')})));
        await submit('media_upload');assert.equal(await page.locator('#media-error').count(),1);assert.equal(await preserved(),draft);
        assert.equal((await query("SELECT count(*)::int n FROM media_library WHERE folder_id=$1 AND name LIKE 'overflow-%'",[nested])).rows[0].n,0);
        const filename=`${currentTheme}-${locale}-${editor}.png`;
        await page.locator('[name=picker_image]').setInputFiles([{name:filename,mimeType:'image/png',buffer:require('node:fs').readFileSync('crates/web/tests/fixtures/avatar.png')},{name:'second-'+filename,mimeType:'image/png',buffer:require('node:fs').readFileSync('crates/web/tests/fixtures/avatar.png')}]);
        await submit('media_upload');assert.equal(await page.locator('#media-error').count(),0);
        assert.equal(await preserved(),draft);
        const asset=(await query('SELECT m.id FROM media_library l JOIN media_objects m ON m.key=l.key WHERE l.folder_id=$1 AND l.name=$2',[nested,filename])).rows[0];assert.ok(asset);
        await page.mouse.move(0,0);const chosen=page.locator(`[value="choose_${asset.id}"]`);assert.equal((await chosen.innerText()).trim(),locale==='it'?'Seleziona':'Select');
        await whiteText(chosen);
        // Opening the preview must remain in the selection flow, including without JS.
        await page.locator('.media-filters [name=media_query]').fill(filename);
        await page.locator('#media-format').selectOption('png');
        await page.locator('#media-sort').selectOption('newest');
        await submit('media_search');
        const tabs = page.context().pages().length;
        const detailResponse = page.context().waitForEvent('response', {predicate:r=>new URL(r.url()).pathname===`/admin/media/picker/${asset.id}`||new URL(r.url()).pathname===`/admin/media/${asset.id}`});
        await page.locator(`[data-media-id="${asset.id}"] .media-preview`).focus();await page.keyboard.press('Enter');
        assert.equal((await detailResponse).status(),200);await page.waitForLoadState('load');
        assert.equal(page.context().pages().length,tabs,'Preview stays in the current selection, without opening an unrelated archive');
        assert.equal(await page.locator('h1').innerText(),filename);
        assert.equal(await preserved(),draft);
        assert.equal(await page.locator('#media-draft [name=media_query]').inputValue(),filename);
        assert.equal(await page.locator('#media-draft [name=media_folder]').inputValue(),String(nested));
        assert.equal(await page.locator('#media-draft [name=media_format]').inputValue(),'png');
        assert.equal(await page.locator('#media-draft [name=media_sort]').inputValue(),'newest');
        assert.equal(await page.locator('[name=action][value=move]').count(),0,'Details in selection mode cannot mutate archive assets');
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
        await page.screenshot({path:path.join(output,`media-picker-detail-${name}-${currentTheme}-${locale}-${editor}.png`),fullPage:true});
        await submit('media_return');
        assert.equal(await page.locator('main.media-main .media-grid').count(),1);
        assert.equal(await preserved(),draft);
        assert.equal(await page.locator('.media-filters [name=media_query]').inputValue(),filename);
        assert.equal(await page.locator('#media-format').inputValue(),'png');
        assert.equal(await page.locator('#media-sort').inputValue(),'newest');
        assert.equal(await page.locator('#media-draft [name=media_folder]').inputValue(),String(nested));
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
        await page.screenshot({path:path.join(output,`media-picker-${name}-${currentTheme}-${locale}-${editor}.png`),fullPage:true});
        if(currentTheme==='essential') {
          const detail=page.waitForResponse(r=>new URL(r.url()).pathname===`/admin/media/picker/${asset.id}`);
          await page.locator(`[data-media-id="${asset.id}"] .media-preview`).click();assert.equal((await detail).status(),200);await page.waitForLoadState('load');
          assert.equal(await page.locator('h1').innerText(),filename);
        }
        const selectedResponse=page.waitForResponse(r=>r.request().method()==='POST');await chosen.focus();await page.keyboard.press('Enter');assert.equal((await selectedResponse).status(),200);await page.waitForLoadState('load');
        assert.equal(await page.locator(editor==='banner'?'#banner-image':'#settings-logo').inputValue(),String(asset.id));
        assert.equal(await draftInput.inputValue(),draft);
        if(editor==='settings') assert.deepEqual(await page.locator('#checkout-countries').evaluate(e=>[...e.selectedOptions].map(o=>o.value).sort()),['FR','IT']);
        else {assert.equal(await page.locator('#banner-name').inputValue(),'Alt <&> '+draft);assert.equal(await page.locator('#banner-content-en-name').inputValue(),'English '+draft);}
        assert.equal((await query('SELECT count(*)::int n FROM home_banners WHERE label=$1',[draft])).rows[0].n,0);
        assert.notEqual((await query('SELECT name FROM store_settings')).rows[0].name,draft);
        await submit(editor==='banner'?'browse':'browse_logo');await submit('media_close');
        assert.equal(await draftInput.inputValue(),draft);
        assert.equal(await page.locator(editor==='banner'?'#banner-image':'#settings-logo').inputValue(),String(asset.id));
      }
    }
  } finally { await query('UPDATE administrators SET locale=$1',[original]);await query('UPDATE storefront_settings SET theme_id=$1',[theme]); }
};
