// Native disclosures, lazy branches and ordinary links in the isolated demo schemas.
const assert = require('node:assert/strict');
const path = require('node:path');
const fs = require('node:fs');
module.exports = async ({page, query, origin, name, js, output, mediaRoot}) => {
  const rootName = `Tree-${name}`;
  const add = async (parent, folder, location) => (await query('INSERT INTO media_folders(parent_id,name,path) VALUES($1,$2,$3) RETURNING id', [parent, folder, location])).rows[0].id;
  const ghost = await add(null, `Ghost-${name}`, `ghost-${name}`);
  const root = await add(null, rootName, rootName);
  const base = await add(root, 'base', `${rootName}/base`);
  const active = await add(base, 'palette', `${rootName}/base/palette`);
  const variants = await add(root, 'varianti', `${rootName}/varianti`);
  const red = await add(variants, 'rosso', `${rootName}/varianti/rosso`);
  const many = await add(root, 'many', `${rootName}/many`);
  const children = (await query("INSERT INTO media_folders(parent_id,name,path) SELECT $1,'Child '||lpad(n::text,3,'0'),$2||'/child-'||n FROM generate_series(1,105)n RETURNING id", [many, `${rootName}/many`])).rows.map(row=>row.id);
  fs.mkdirSync(path.join(mediaRoot, rootName, 'base', 'palette'), {recursive:true});
  fs.mkdirSync(path.join(mediaRoot, rootName, 'varianti', 'rosso'), {recursive:true});
  for(let n=1;n<=105;n++) fs.mkdirSync(path.join(mediaRoot,rootName,'many',`child-${n}`),{recursive:true});
  const visibleIcon = async link => link.locator(':scope > svg').evaluateAll(icons=>icons.filter(icon=>getComputedStyle(icon).display!=='none').map(icon=>icon.dataset.uiIcon));
  const original = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  try {
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await query('UPDATE administrators SET locale=$1', [locale]);
      await page.goto(origin + `/admin/media?folder=${active}&q=needle&format=pdf&usage=unused&sort=newest`);
      const tree = page.locator('.media-folder-tree');
      assert.equal(await tree.locator(`[data-folder-link="${ghost}"]`).count(),0,'Absent physical directories never appear');
      if(!fs.existsSync(path.join(mediaRoot,'library'))) assert.equal(await tree.getByRole('link',{name:'Media',exact:true}).count(),0);
      assert.equal(await tree.count(), 1, 'Folder navigation must retain the tree, rather than only the current children');
      const rootBranch = tree.locator(`details[data-folder="${root}"]`);
      const baseBranch = tree.locator(`details[data-folder="${base}"]`);
      assert.equal(await rootBranch.getAttribute('open'), '');
      assert.equal(await baseBranch.getAttribute('open'), '');
      assert.equal(await tree.locator('a[aria-current="page"]').textContent().then(s=>s.trim()), 'palette');
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${root}"]`)), ['folder-open'], 'Expanded branches use the open folder icon');
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${active}"]`)), ['folder-open'], 'The selected leaf folder uses the open folder icon');
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${variants}"]`)), ['folder'], 'Collapsed branches use the closed folder icon');
      const summary = rootBranch.locator(':scope > summary');
      await summary.focus(); await page.keyboard.press('Space');
      assert.equal(await rootBranch.getAttribute('open'), null);
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${root}"]`)), ['folder']);
      await page.keyboard.press('Enter');
      assert.equal(await rootBranch.getAttribute('open'), '');
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${root}"]`)), ['folder-open']);
      assert.notEqual(await summary.evaluate(e=>getComputedStyle(e).outlineStyle), 'none');
      const branch = tree.locator(`details[data-folder="${variants}"]`);
      const before = page.url();
      await branch.locator(':scope > summary').focus(); await page.keyboard.press('Enter');
      assert.deepEqual(await visibleIcon(tree.locator(`[data-folder-link="${variants}"]`)), ['folder-open']);
      if (js) {
        await branch.locator(`[data-folder-link="${red}"]`).waitFor();
        assert.equal(page.url(), before, 'Expanding a branch keeps the current files and filters');
      } else {
        await branch.locator('.media-tree-fallback').click();
        await page.waitForURL(url=>url.searchParams.get('folder')===String(variants));
      }
      const redLink = tree.locator(`[data-folder-link="${red}"]`);
      assert.equal(await redLink.isVisible(), true);
      assert.deepEqual(await visibleIcon(redLink), ['folder'], 'Unselected leaf folders stay closed');
      const target = new URL(await redLink.getAttribute('href'), origin);
      for (const [key,value] of [['q','needle'],['format','pdf'],['usage','unused'],['sort','newest']]) assert.equal(target.searchParams.get(key), value);
      await redLink.click(); await page.waitForURL(url=>url.searchParams.get('folder')===String(red));
      assert.equal(await tree.locator('a[aria-current="page"]').textContent().then(s=>s.trim()), 'rosso');
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
      await page.locator('.media-folders').screenshot({path:path.join(output,`media-tree-${name}-${theme}-${locale}.png`)});
      await tree.locator('.media-folder-root').click();
      await page.waitForURL(url=>!url.searchParams.has('folder'));
      assert.equal(await tree.locator('.media-folder-root').getAttribute('aria-current'), 'page');
      await tree.getByRole('link', {name:rootName, exact:true}).click();
      await page.waitForURL(url=>url.searchParams.get('folder')===String(root));
      assert.equal(await tree.locator(`[data-folder-link="${root}"]`).getAttribute('aria-current'), 'page', 'The name inside a disclosure navigates to that folder');
      await tree.locator(`[data-folder-link="${many}"]`).click();
      await page.waitForURL(url=>url.searchParams.get('folder')===String(many));
      const pagination = tree.getByRole('navigation', {name:locale==='it'?'Pagine cartelle':'Folder pages', exact:true});
      assert.match(await pagination.innerText(), /1–100/);
      for (const button of await pagination.locator('.pagination-button').all()) assert.ok((await button.boundingBox()).height>=44);
      await pagination.getByRole('link', {name:locale==='it'?'Successivo':'Next',exact:true}).focus();
      await page.keyboard.press('Enter');
      await page.waitForURL(url=>url.searchParams.get('folders_page')==='2');
      assert.equal(new URL(page.url()).searchParams.get('q'), 'needle');
      assert.match(await pagination.innerText(), /101–105/);
      await pagination.getByRole('link', {name:locale==='it'?'Precedente':'Previous',exact:true}).click();
      await page.waitForURL(url=>url.searchParams.get('folders_page')==='1');
      assert.match(await pagination.innerText(), /1–100/);
      await page.goto(origin+'/admin/components');
      const catalogue=page.locator('.ui-icon-catalog');
      assert.equal(await catalogue.locator('[data-ui-icon="folder"]').count(),1);
      assert.equal(await catalogue.locator('[data-ui-icon="folder-open"]').count(),1);
      assert.equal(await catalogue.getByText(locale==='it'?'Cartella chiusa':'Closed folder',{exact:true}).count(),1);
      assert.equal(await catalogue.getByText(locale==='it'?'Cartella aperta':'Open folder',{exact:true}).count(),1);
    }
  } finally {
    await query('UPDATE administrators SET locale=$1', [original]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
    fs.rmSync(path.join(mediaRoot,rootName),{recursive:true,force:true});
    // The fixture has no files or relationships outside its own subtree.
    await query('DELETE FROM media_folders WHERE id=ANY($1)', [[...children, many, red, variants, active, base, root, ghost]]);
  }
};
