// Only synthetic uploads in the isolated demo schema are permanently removed.
const assert = require('node:assert/strict');
const path = require('node:path');

module.exports = async ({page, query, origin, name, output}) => {
  const originalLocale = (await query('SELECT locale FROM administrators LIMIT 1')).rows[0].locale;
  const originalTheme = (await query('SELECT theme_id FROM storefront_settings')).rows[0].theme_id;
  try {
    for (const theme of ['default', 'essential']) for (const locale of ['it', 'en']) {
      await query('UPDATE storefront_settings SET theme_id=$1', [theme]);
      await query('UPDATE administrators SET locale=$1', [locale]);
      const italian = locale === 'it';
      const filename = `trash-${name}-${theme}-${locale}.png`;
      await page.goto(origin + '/admin/media');
      await page.locator('summary').filter({hasText: italian ? /^Carica file$/ : /^Upload files$/}).click();
      await page.getByLabel(italian ? 'File da caricare' : 'Files to upload', {exact:true}).setInputFiles({
        name: filename, mimeType: 'image/png', buffer: require('node:fs').readFileSync('crates/web/tests/fixtures/avatar.png'),
      });
      await page.getByRole('button', {name: italian ? 'Carica nella mediateca' : 'Upload to media library', exact:true}).click();
      await page.waitForURL(url => url.pathname === '/admin/media' && url.searchParams.has('selected'));
      const id = Number(new URL(page.url()).searchParams.get('selected'));
      assert.ok(id > 0);
      await page.goto(origin + `/admin/media/${id}`);
      await page.getByLabel(italian ? 'Confermo lo spostamento nel cestino' : 'I confirm moving to the trash', {exact:true}).check();
      await page.getByRole('button', {name: italian ? 'Sposta nel cestino' : 'Move to trash', exact:true}).click();
      await page.getByRole('button', {name: italian ? 'Ripristina immagine' : 'Restore image', exact:true}).waitFor();
      await page.goto(origin + '/admin/media?trash=true');
      assert.equal(await page.locator(`[data-media-id="${id}"]`).count(), 1);
      const form = page.locator('form[action="/admin/media/purge"]');
      const checkbox = form.getByRole('checkbox');
      const confirmation = italian ? 'Confermo l’eliminazione definitiva di tutti i file nel cestino' : 'I confirm permanent deletion of all files in the trash';
      assert.equal(await form.getByLabel(confirmation, {exact:true}).count(), 1);
      assert.equal(await checkbox.getAttribute('required'), '');
      assert.equal(await checkbox.isChecked(), false);
      assert.equal(await form.evaluate(element => element.checkValidity()), false);
      await checkbox.focus();
      await page.keyboard.press('Space');
      assert.equal(await checkbox.isChecked(), true);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      await page.screenshot({path:path.join(output, `media-trash-${name}-${theme}-${locale}.png`), fullPage:true});
      await form.getByRole('button', {name:italian ? 'Svuota cestino' : 'Empty trash', exact:true}).focus();
      const response = page.waitForResponse(r => new URL(r.url()).pathname === '/admin/media/purge' && r.request().method() === 'POST');
      await page.keyboard.press('Enter');
      assert.equal((await response).status(), 303);
      await page.waitForURL(url => url.pathname === '/admin/media' && url.searchParams.get('saved') === 'true');
      assert.equal(await page.locator(`[data-media-id="${id}"]`).count(), 0);
      assert.equal((await query('SELECT count(*) FROM media_library l JOIN media_objects m ON m.key=l.key WHERE m.id=$1', [id])).rows[0].count, '0',
        'A file trashed moments ago is removed without backdating timestamps');
    }
  } finally {
    await query('UPDATE administrators SET locale=$1', [originalLocale]);
    await query('UPDATE storefront_settings SET theme_id=$1', [originalTheme]);
  }
};
