// Real background discovery: copied sources of any provenance, no application data.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
module.exports = async ({page, query, origin, name, output, mediaRoot}) => {
  const directory = `FTP-${name}`;
  fs.mkdirSync(path.join(mediaRoot, directory, 'Empty'), {recursive:true});
  fs.copyFileSync('crates/web/tests/fixtures/avatar.png', path.join(mediaRoot, directory, 'picture.png'));
  fs.writeFileSync(path.join(mediaRoot, `root-${name}.pdf`), '%PDF-1.4\nSynthetic document\n%%EOF\n');
  fs.writeFileSync(path.join(mediaRoot, directory, 'notes.txt'), 'Synthetic UTF-8 document.');
  for (const excluded of ['cache/copied.png', 'staging/copied.png']) {
    fs.mkdirSync(path.dirname(path.join(mediaRoot, excluded)), {recursive:true});
    fs.copyFileSync('crates/web/tests/fixtures/avatar.png', path.join(mediaRoot, excluded));
  }
  const names = [`root-${name}.pdf`, `${directory}/picture.png`, `${directory}/notes.txt`];
  let found = [];
  for (let attempt = 0; attempt < 100; attempt++) {
    found = (await query('SELECT m.id,p.path FROM media_locations p JOIN media_objects m ON m.key=p.key JOIN media_library l ON l.key=m.key WHERE p.path=ANY($1)', [names])).rows;
    if (found.length === names.length) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.equal(found.length, names.length, 'Files copied directly into storage/media must appear automatically');
  assert.equal((await query('SELECT count(*) FROM media_folders WHERE path=$1', [`${directory}/Empty`])).rows[0].count, '1', 'Empty copied directories are navigable');
  assert.equal((await query("SELECT count(*) FROM media_locations WHERE path IN ('cache/copied.png','staging/copied.png')")).rows[0].count,'0');
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  const original = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  try {
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await query('UPDATE administrators SET locale=$1', [locale]);
      await page.goto(origin + '/admin/media?q=' + encodeURIComponent(`root-${name}`));
      assert.equal(await page.locator('.media-card').count(), 1);
      assert.equal(await page.locator('nav.shared-pagination').last().getAttribute('aria-label'),locale === 'it' ? 'Paginazione file' : 'File pagination');
      assert.equal(await page.locator('.media-card img').count(), 0, 'PDF uses a document preview');
      await page.screenshot({path:path.join(output, `media-files-grid-${name}-${theme}-${locale}.png`), fullPage:true});
      await page.locator('.media-preview').click();
      const downloadBox = await page.locator('.media-detail-copy .media-actions a[href$="/download"]').boundingBox();
      const usageBox = await page.locator('.media-usage h2').boundingBox();
      const previewBox = await page.locator('.media-detail-preview').boundingBox();
      const usageGap = usageBox.y - downloadBox.y - downloadBox.height;
      assert.ok(usageGap >= 15.5 && previewBox.height <= 350, `Document layout needs separate actions and a compact preview: gap ${usageGap}, height ${previewBox.height}`);
      const pdf = found.find(item => item.path === `root-${name}.pdf`);
      const download = await page.request.get(origin + `/admin/media/${pdf.id}/download`);
      assert.equal(download.status(), 200);
      assert.match(download.headers()['content-disposition'], /^attachment/);
      assert.equal((await download.body()).toString(), fs.readFileSync(path.join(mediaRoot, `root-${name}.pdf`), 'utf8'));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      await page.screenshot({path:path.join(output, `media-files-${name}-${theme}-${locale}.png`), fullPage:true});
      const form = page.locator('form').filter({has:page.locator('#media-name')});
      await form.getByLabel(locale === 'it' ? 'Nome del file' : 'File name', {exact:true}).fill(`copy-${theme}-${locale}.pdf`);
      await form.locator('[name=destination]').fill(directory);
      await form.locator('button[value=copy]').focus(); await page.keyboard.press('Enter');
      await page.waitForURL(url => url.pathname !== `/admin/media/${pdf.id}` && url.searchParams.get('saved') === 'true');
      const copyId = Number(new URL(page.url()).pathname.split('/').pop());
      assert.notEqual(copyId, Number(pdf.id));
      await page.locator('#media-name').fill(`renamed-${theme}-${locale}.pdf`);
      await page.locator('#media-destination').fill('/');
      await page.locator('button[value=move]').click();
      await page.waitForURL(url => url.pathname === `/admin/media/${copyId}` && url.searchParams.get('saved') === 'true');
      assert.equal((await query('SELECT p.path FROM media_locations p JOIN media_objects m ON m.key=p.key WHERE m.id=$1',[copyId])).rows[0].path,`renamed-${theme}-${locale}.pdf`);
      await page.locator('input[name=confirm]').check();
      await page.locator('button[value=trash]').click();
      await page.locator('button[value=restore]').waitFor();
      await page.locator('button[value=restore]').click();
      await page.locator('button[value=trash]').waitFor();
    }
  } finally {
    await query('UPDATE administrators SET locale=$1', [original]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
  }
};
