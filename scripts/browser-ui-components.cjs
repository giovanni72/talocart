// Runs in the demo suite's isolated schema, on desktop/mobile with and without JS.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page, origin, name, js, output}) => {
  if (js) await require('./browser-tabs.cjs')(page.context().browser());
  const viewport = page.viewportSize();
  await page.goto(origin + '/admin');
  await page.getByRole('link', {name:'Componenti dell’interfaccia',exact:true}).click();
  await page.waitForURL('**/admin/components');
  const label = page.getByLabel('Nome di esempio', {exact:true});
  const state = page.getByLabel('Stato di esempio', {exact:true});
  assert.equal(await page.getByLabel('Sola lettura', {exact:true}).getAttribute('readonly'), '');
  assert.equal(await page.getByLabel('Campo non disponibile', {exact:true}).isDisabled(), true);
  assert.equal(await page.getByRole('button', {name:'Non disponibile',exact:true}).isDisabled(), true);
  await label.fill('Nuovo esempio');
  await page.getByRole('button', {name:'Ripristina campi',exact:true}).click();
  assert.equal(await label.inputValue(), 'Esempio di prodotto');
  await label.fill('Caffè & tazze'); await state.selectOption('inactive');
  await page.getByRole('button', {name:'Aggiorna anteprima',exact:true}).click();
  await page.waitForURL(url => url.searchParams.get('label') === 'Caffè & tazze');
  assert.equal(await label.inputValue(), 'Caffè & tazze');
  assert.equal(await state.inputValue(), 'inactive');
  assert.equal(await page.locator('#preview-result [role]').getAttribute('role'), 'status');
  await label.fill('');
  await page.getByRole('button', {name:'Aggiorna anteprima',exact:true}).click();
  assert.equal(await label.evaluate(e => e === document.activeElement && !e.checkValidity()), true, 'Native required validation focuses the field');
  const response = await page.goto(origin + '/admin/components?preview=1&label=%20&status=bad');
  assert.equal(response.status(), 422);
  for (const id of ['example-label', 'example-status', 'example-invalid']) {
    const control = page.locator('#' + id);
    assert.equal(await control.getAttribute('aria-invalid'), 'true');
    const ids = (await control.getAttribute('aria-describedby')).split(/\s+/);
    for (const described of ids) assert.equal(await page.locator('#' + described).count(), 1, 'Descriptions reference existing elements');
  }
  assert.equal(await page.locator('#preview-result [role]').getAttribute('role'), 'alert');
  await page.getByRole('link', {name:'Ripristina esempi',exact:true}).click();
  await page.waitForURL(url => url.pathname === '/admin/components' && !url.search);
  const compact = page.getByRole('link', {name:'Compatto',exact:true});
  const compactMetrics = await compact.evaluate(button => {
    const style = getComputedStyle(button);
    return { top: style.paddingTop, bottom: style.paddingBottom, horizontal: style.paddingLeft, height: button.getBoundingClientRect().height };
  });
  assert.deepEqual(compactMetrics, {top: '4px', bottom: '4px', horizontal: '8px', height: 36}, 'Shared compact buttons have reduced vertical padding and height');
  const mini = page.getByRole('link',{name:'Mini',exact:true});
  const miniIcon = page.getByRole('link',{name:'Icona mini',exact:true});
  assert.equal(await mini.evaluate(e=>e.getBoundingClientRect().height),28);
  assert.deepEqual(await miniIcon.evaluate(e=>{const r=e.getBoundingClientRect();return [r.width,r.height];}),[28,28]);
  assert.equal((await miniIcon.textContent()).trim(),'');
  await compact.focus(); await page.keyboard.press('Tab');
  assert.ok(await mini.evaluate(e=>e===document.activeElement));
  await page.keyboard.press('Tab');
  assert.ok(await miniIcon.evaluate(e=>e===document.activeElement));
  await page.keyboard.press('Tab');
  assert.equal(await page.locator('.media-folder-root').evaluate(e => e === document.activeElement), true, 'Tab skips the unavailable button and reaches the folder navigation examples');
  await page.keyboard.press('Tab');
  assert.equal(await page.locator('.media-folder-link').evaluate(e => e === document.activeElement), true, 'Folder navigation remains keyboard accessible');
  await page.getByRole('link', {name:'Vai ai campi di esempio',exact:true}).focus(); await page.keyboard.press('Tab');
  assert.equal(await label.evaluate(e => e === document.activeElement), true, 'Decorative icons add no keyboard stop');
  // Shared tab menu: native links without JS, ARIA tabs over in-page sections with JS.
  const sections = page.getByRole('navigation', {name:'Sezioni della scheda cliente',exact:true});
  const panelIds = ['profile','billing','addresses','orders','access'].map(key => '#example-tab-' + key);
  const visiblePanels = () => page.evaluate(ids => ids.filter(id => !document.querySelector(id).hidden), panelIds);
  assert.equal(await page.getByRole('navigation', {name:'Sezioni delle impostazioni',exact:true}).locator('[aria-current=page]').innerText(), 'Generali');
  if (js) {
    const tab = name => sections.getByRole('tab', {name, exact:false});
    assert.equal(await sections.getByRole('tab').count(), 5);
    assert.deepEqual(await visiblePanels(), ['#example-tab-profile']);
    assert.equal(await tab('Anagrafica').getAttribute('aria-selected'), 'true');
    await tab('Fatturazione').click();
    assert.deepEqual(await visiblePanels(), ['#example-tab-billing']);
    assert.equal(new URL(page.url()).hash, '#example-tab-billing');
    assert.equal(await page.locator('#example-tab-billing').getAttribute('aria-labelledby'), await tab('Fatturazione').getAttribute('id'));
    await tab('Fatturazione').focus(); await page.keyboard.press('ArrowRight');
    assert.ok(await tab('Rubrica indirizzi').evaluate(e => e === document.activeElement && e.getAttribute('aria-selected') === 'true'));
    await page.keyboard.press('End');
    assert.deepEqual(await visiblePanels(), ['#example-tab-access']);
    await page.keyboard.press('ArrowRight');
    assert.ok(await tab('Anagrafica').evaluate(e => e === document.activeElement), 'Arrow keys wrap around');
    await page.keyboard.press('Tab');
    assert.ok(await page.locator('#example-tab-profile').evaluate(e => e === document.activeElement), 'Only the selected tab is a tab stop');
    await page.goto(origin + '/admin/components#example-tab-orders');
    assert.deepEqual(await visiblePanels(), ['#example-tab-orders'], 'The address selects the section');
  } else {
    assert.equal(await sections.getByRole('tab').count(), 0);
    assert.deepEqual(await visiblePanels(), panelIds, 'Without JS every section stays visible');
    assert.equal(await sections.getByRole('link').count(), 5);
  }
  for (const width of [320,390,768,1440]) {
    await page.setViewportSize({width,height:1000});
    for (const route of ['/admin/components', '/admin/catalog/categories?q=assente-ui&status=active&saved=1', '/catalog?brand=assente-ui']) {
      await page.goto(origin + route);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `No overflow ${width} ${route}`);
      const controls = page.locator('.ui-control:visible');
      for (const control of await controls.all()) {
        assert.equal(await control.evaluate(e => e.labels.length > 0), true, 'Native controls have labels');
        const box = await control.boundingBox(); assert.ok(box.height >= 44 && box.x >= 0 && box.x + box.width <= width);
      }
      const buttons = page.locator('.ui-button:visible');
      for (const button of await buttons.all()) {
        const folderNavigation = await button.evaluate(e => e.matches('.ui-button--navigation.media-folder-root,.ui-button--navigation.media-folder-link'));
        // Rich text editor tools are the dense toolbar exception of ui-contract.cjs.
        const richTool = await button.evaluate(e => e.matches('.ui-rich-text .ui-rich-tool'));
        const minimumHeight = folderNavigation ? (width <= 600 ? 40 : 36) : await button.evaluate(e => e.matches('.ui-rich-text .ui-rich-tool,.ui-button--mini') ? 28 : e.classList.contains('ui-button--compact') ? 36 : 44);
        const size = await button.boundingBox(); assert.ok(size.height + 0.01 >= minimumHeight && size.x >= 5 && size.x + size.width + 5 <= width, `Action fits viewport ${width} ${route} ${await button.getAttribute('class')} ${JSON.stringify(size)}`);
        const style = await button.evaluate(e => {
          const s=getComputedStyle(e), ring=getComputedStyle(e,'::after');
          return {inner:parseFloat(s.borderTopWidth),outer:parseFloat(ring.borderTopWidth),inset:parseFloat(ring.top),pointer:ring.pointerEvents};
        });
        if (await button.evaluate(e => e.matches('.ui-button--plain,.ui-button--navigation'))) {
          assert.equal(style.inner, 0);
          assert.equal(await button.evaluate(e => getComputedStyle(e,'::after').content), 'none');
        } else {
          assert.equal(style.inner, 2); assert.equal(style.outer, 1); assert.ok(style.inset <= -4); assert.equal(style.pointer,'none');
        }
        if ((await button.getAttribute('aria-disabled')) !== 'true' && !(await button.isDisabled())) {
          await button.focus();
          const focus = await button.evaluate(e => ({active:e === document.activeElement,outline:getComputedStyle(e).outlineStyle,offset:parseFloat(getComputedStyle(e).outlineOffset)}));
          assert.ok(focus.active && focus.outline === 'solid' && (folderNavigation ? focus.offset === 2 : richTool ? focus.offset >= 1 : focus.offset > 5), `Focus follows the shared action/navigation variant: ${width} ${route} ${await button.innerText()} ${JSON.stringify(focus)}`);
        }
      }
      if (route.includes('/catalog?')) {
        assert.equal(await page.locator('.ui-empty').count(), 1);
        assert.equal(await page.locator('[data-catalog-page-apply]').isVisible(), !js);
      }
      if ([320,1440].includes(width)) {
        await page.locator('main').screenshot({path:path.join(output, `ui-${route.includes('/components')?'reference':route.includes('/admin/')?'admin':'store'}-${name}-${width}.png`)});
        if (route === '/admin/components') await page.locator('.ui-example-actions').first().screenshot({path:path.join(output,`ui-buttons-${name}-${width}.png`)});
        if (route === '/admin/components') await page.locator('#tabs').screenshot({path:path.join(output,`ui-tabs-${name}-${width}.png`)});
      }
    }
  }
  await page.setViewportSize(viewport);
  await page.goto(origin + '/admin/catalog/categories');
  await page.getByLabel('Cerca per nome o slug', {exact:true}).fill('assente-ui');
  await page.getByLabel('Stato', {exact:true}).selectOption('inactive');
  await page.getByRole('button', {name:'Filtra',exact:true}).click();
  await page.waitForURL(url => url.searchParams.get('q') === 'assente-ui' && url.searchParams.get('status') === 'inactive');
  assert.equal(await page.locator('.ui-empty').count(), 1);
  await page.getByRole('link', {name:'Azzera filtri',exact:true}).click();
  await page.waitForURL('**/admin/catalog/categories');
  assert.equal(await page.getByLabel('Cerca per nome o slug', {exact:true}).inputValue(), '');
  // Shared pagination remains navigable with the same double-border controls.
  await page.goto(origin + '/catalog?per_page=21');
  await page.getByRole('navigation', {name:'Pagine catalogo'}).getByRole('link', {name:'Successivo',exact:true}).focus();
  await page.keyboard.press('Enter');
  await page.waitForURL(url => url.searchParams.get('page') === '2');
  assert.equal(await page.locator('.store-product').count(), 6);
  await page.goto(origin + '/admin');
};
