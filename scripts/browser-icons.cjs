// Uses the demo suite's isolated shop and the real account language form.
const assert = require('node:assert/strict');
const path = require('node:path');
async function assertIcon(icon, size = 20) {
  assert.equal(await icon.count(), 1);
  const actual = await icon.evaluate(e => {
    const s = getComputedStyle(e), box = e.getBoundingClientRect();
    return {hidden:e.getAttribute('aria-hidden'),focusable:e.getAttribute('focusable'),
      stroke:e.getAttribute('stroke'),colour:s.color,parentColour:getComputedStyle(e.parentElement).color,
      width:box.width,height:box.height,namespace:e.namespaceURI,tabindex:e.getAttribute('tabindex')};
  });
  assert.equal(actual.hidden, 'true'); assert.equal(actual.focusable, 'false');
  assert.equal(actual.tabindex, null); assert.equal(actual.stroke, 'currentColor');
  assert.equal(actual.colour, actual.parentColour);
  assert.equal(actual.namespace, 'http://www.w3.org/2000/svg');
  assert.equal(actual.width, size); assert.equal(actual.height, size);
}
module.exports = async ({page,origin,name,output}) => {
  const languageNames = {it:'Salva lingua',en:'Save language'};
  for (const locale of ['en','it']) {
    await page.goto(origin+'/admin/account');
    const previous = await page.locator('html').getAttribute('lang');
    await page.locator('#account-language').selectOption(locale);
    await page.getByRole('button',{name:languageNames[previous],exact:true}).click();
    await page.waitForURL('**/admin/account?saved=1');
    await page.goto(origin+'/admin/components');
    assert.equal(await page.locator('html').getAttribute('lang'),locale);
    const title=locale==='it'?'Icone Lucide':'Lucide icons';
    assert.ok(await page.getByRole('heading',{name:title,exact:true}).isVisible());
    const catalog=page.locator('.ui-icon-catalog');
    assert.equal(await catalog.getByRole('listitem').count(),66);
    for (const icon of await catalog.locator('svg').all()) await assertIcon(icon);
    const edit=page.getByRole('link',{name:locale==='it'?'Modifica esempio':'Edit example',exact:true});
    const compact=page.getByRole('link',{name:locale==='it'?'Mostra messaggi':'View messages',exact:true});
    const iconOnly=page.getByRole('link',{name:locale==='it'?'Vai ai campi di esempio':'Go to example fields',exact:true});
    const miniIcon=page.getByRole('link',{name:locale==='it'?'Icona mini':'Mini icon',exact:true});
    assert.deepEqual(await miniIcon.evaluate(e=>{const r=e.getBoundingClientRect();return [r.width,r.height];}),[28,28]);
    assert.equal(await page.getByRole('link',{name:'Mini',exact:true}).evaluate(e=>e.getBoundingClientRect().height),28);
    await assertIcon(miniIcon.locator('svg'),16);
    await page.locator('.ui-example-actions').first().screenshot({path:path.join(output,`mini-buttons-${name}-${locale}.png`)});

    await assertIcon(edit.locator('svg')); await assertIcon(compact.locator('svg'),16);
    await assertIcon(iconOnly.locator('svg'));
    const box=await iconOnly.boundingBox(); assert.equal(box.width,box.height); assert.ok(box.height>=44);
    await edit.focus(); await page.keyboard.press('Tab');
    assert.ok(await compact.evaluate(e=>e===document.activeElement),'SVG introduces no tab stop');
    await page.keyboard.press('Tab');assert.ok(await iconOnly.evaluate(e=>e===document.activeElement));
    assert.equal(await iconOnly.evaluate(e=>getComputedStyle(e).outlineStyle),'solid');
    await page.keyboard.press('Enter');await page.waitForURL('**/admin/components#fields');
    assert.ok(await page.getByRole('heading',{name:locale==='it'?'Input e select':'Inputs and selects',exact:true}).isVisible());
    await page.goto(origin+'/admin/components');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    await page.locator('#icons').screenshot({path:path.join(output,`icons-${name}-${locale}.png`)});
  }
};
module.exports.assertIcon = assertIcon;
