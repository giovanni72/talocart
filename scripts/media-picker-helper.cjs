const assert = require('node:assert/strict');
const path = require('node:path');
// Exercises the real shared picker, its upload and the separate attachment POST.
module.exports = async (page, link, file) => {
  const branding = ['Seleziona logo', 'Seleziona favicon'].includes(link) && await page.locator('#settings-editor').count();
  const kind = link.endsWith('favicon') ? 'favicon' : 'logo';
  if (branding) {await page.goto(new URL('/admin/media', page.url()).href);await page.locator('.media-upload summary').click();}
  else await page.getByRole('link', { name: link, exact: true }).click();
  await page.getByLabel('File da caricare', { exact: true }).setInputFiles(typeof file === 'string' ? path.resolve(file) : file);
  const posted = page.waitForResponse(r => new URL(r.url()).pathname === '/admin/media/upload' && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Carica nella mediateca', exact: true }).click();
  const response = await posted;
  if (response.status() !== 303) { await page.getByRole('alert').waitFor(); return response.status(); }
  await page.waitForURL(url => url.pathname === '/admin/media' && url.searchParams.has('selected'));
  if (branding) {
    const id = new URL(page.url()).searchParams.get('selected');
    const uploadedName = await page.locator('.media-card-new .media-card-copy strong').innerText();
    await page.goto(new URL('/admin/settings/appearance', page.url()).href);
    await page.locator(`button[value=browse_${kind}]`).click();
    await page.locator('#settings-media-query').fill(uploadedName);
    await page.locator('button[value=media_search]:visible').click();
    await page.locator(`button[value=choose_${id}]`).click();
    await page.locator('#settings-save').click();
    await page.waitForURL('**/admin/settings/appearance?saved=1');
    return response.status();
  }
  const selected = page.locator('.media-card-new').getByRole('button', { name: /^Seleziona / });
  const attached = page.waitForResponse(r => new URL(r.url()).pathname === '/admin/media/select' && r.request().method() === 'POST');
  await selected.click(); assert.equal((await attached).status(), 303);
  await page.waitForURL(url => !url.pathname.startsWith('/admin/media'));
  return response.status();
};
