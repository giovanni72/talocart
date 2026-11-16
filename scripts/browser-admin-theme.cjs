// Uses the demo suite's disposable schema, native forms and the existing browser profiles.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({ page, query, origin, output, name, js }) => {
  const viewport = page.viewportSize();
  const original = (await query('SELECT palette,typography,density,public_origin FROM storefront_settings WHERE id=1')).rows[0];
  const expected = {
    forest: { paper: '#f6f5ef', ink: '#243e32', white: '#fffef9', accent: '#dce7ae' },
    ocean: { paper: '#f1f5f7', ink: '#203e50', white: '#fcfeff', accent: '#c7e3e7' },
    clay: { paper: '#faf3ed', ink: '#593729', white: '#fffdf9', accent: '#f1d6be' },
  };
  const navigate = async route => { const r = await page.goto(origin + route); assert.equal(r.status(), 200, route); };
  const save = async (palette, typography, density) => {
    await navigate('/admin/settings/storefront');
    await page.getByLabel('Colori', { exact: true }).selectOption(palette);
    await page.getByLabel('Caratteri', { exact: true }).selectOption(typography);
    await page.getByLabel('Densità', { exact: true }).selectOption(density);
    await page.getByRole('button', { name: 'Salva impostazioni', exact: true }).click();
    await page.waitForURL('**/admin/settings/storefront?saved=1');
    await page.locator('#settings-saved').waitFor();
  };
  const theme = async () => page.evaluate(() => {
    const css = getComputedStyle(document.body);
    return { palette: document.body.dataset.palette, typography: document.body.dataset.typography, density: document.body.dataset.density,
      paper: css.getPropertyValue('--paper').trim(), ink: css.getPropertyValue('--ink').trim(), white: css.getPropertyValue('--white').trim(), accent: css.getPropertyValue('--accent').trim(),
      display: css.getPropertyValue('--display').trim(), headingFont: getComputedStyle(document.querySelector('h1')).fontFamily, background: css.backgroundColor, color: css.color };
  });
  const contrast = async () => page.evaluate(() => {
    const parse = color => {
      const values = color.match(/[\d.]+/g).map(Number);
      const rgb = values.slice(0,3).map(v=>color.startsWith('color(srgb') ? v : v/255);
      return [...rgb,values[3] ?? 1];
    };
    const background = el => {
      const rgba=parse(getComputedStyle(el).backgroundColor);
      const base=el.parentElement ? background(el.parentElement) : [1,1,1];
      return rgba.slice(0,3).map((v,i)=>v*rgba[3]+base[i]*(1-rgba[3]));
    };
    const luminance = rgb => {
      const values=rgb.map(v=>v<=.04045 ? v/12.92 : ((v+.055)/1.055)**2.4);
      return .2126*values[0]+.7152*values[1]+.0722*values[2];
    };
    return ['.admin-main', '.ui-page-header h1', '.admin-sidebar', '.sidebar-label', '.admin-sidebar nav a.active', '.sidebar-bottom', '.ui-control', '.ui-button', '.ui-message--success'].flatMap(selector => {
      const el = document.querySelector(selector); if (!el || !el.getClientRects().length) return [];
      const a=luminance(parse(getComputedStyle(el).color).slice(0,3)), b=luminance(background(el));
      return [{selector,ratio:(Math.max(a,b)+.05)/(Math.min(a,b)+.05)}];
    });
  });
  const densities = {};
  try {
    for (const palette of ['forest','ocean','clay']) for (const typography of ['editorial','modern']) for (const density of ['comfortable','compact']) {
      await save(palette, typography, density);
      const admin = await theme();
      assert.deepEqual({palette:admin.palette,typography:admin.typography,density:admin.density},{palette,typography,density});
      for (const key of ['paper','ink','white','accent']) assert.equal(admin[key],expected[palette][key],`${palette} ${key}`);
      const ratios = await contrast(); assert.ok(ratios.every(r=>r.ratio>=4.5),JSON.stringify(ratios));
      await navigate('/'); const store=await theme();
      for (const key of ['paper','ink','white','accent','display','headingFont','background','color']) assert.equal(admin[key],store[key],`Shared ${key}`);
      assert.equal(admin.display.includes('Iowan'),typography==='editorial');
      assert.equal(admin.headingFont.includes('Iowan'),typography==='editorial');
      for (const route of ['/admin','/admin/catalog/products','/admin/catalog/brands','/admin/components','/admin/media','/admin/account','/admin/settings/storefront']) {
        await navigate(route); const current=await theme(); assert.deepEqual(current,admin,route);
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`No overflow ${route}`);
      }
      if (palette==='ocean' && typography==='editorial' && density==='comfortable') await page.screenshot({path:path.join(output,`admin-theme-editorial-${name}.png`),fullPage:true});
      await page.getByLabel('Colori',{exact:true}).focus(); await page.keyboard.press('Tab');
      assert.ok(await page.getByLabel('Caratteri',{exact:true}).evaluate(e=>e===document.activeElement && getComputedStyle(e).outlineStyle==='solid'));
      if (palette==='ocean' && typography==='modern') {
        const form=await page.getByLabel('Colori',{exact:true}).boundingBox();
        const button=await page.getByRole('button',{name:'Salva impostazioni',exact:true}).boundingBox();
        await navigate('/admin/catalog/products');
        const cell=await page.locator('.product-table tbody td').first().evaluate(e=>parseFloat(getComputedStyle(e).paddingTop));
        const table=await page.locator('.product-table').boundingBox();
        const main=await page.locator('.admin-main').evaluate(e=>parseFloat(getComputedStyle(e).paddingTop));
        const sidebar=await page.locator('.admin-sidebar nav a.active').evaluate(e=>e.getBoundingClientRect().height);
        const compact=await page.locator('.ui-button--compact').first().evaluate(e=>({height:getComputedStyle(e).minHeight,padding:getComputedStyle(e).paddingTop}));
        assert.deepEqual(compact,{height:'36px',padding:'4px'});
        await navigate('/admin/catalog/brands');
        const card=await page.locator('.taxonomy-card').first().evaluate(e=>parseFloat(getComputedStyle(e).paddingTop));
        densities[density]={form:form.height,button:button.height,cell,table:table.height,main,sidebar,card};
        await navigate('/admin/catalog/products');
        await page.screenshot({path:path.join(output,`admin-theme-density-${density}-${name}.png`),fullPage:true});
      }
      if (typography==='modern' && density==='compact') {
        for (const width of [320,390,768,1440,1920,2560]) {
          await page.setViewportSize({width,height:1050});
          for (const route of ['/admin/settings/storefront','/admin/catalog/products','/admin/catalog/categories','/admin/components','/admin/login','/admin/reset']) {
            await navigate(route); assert.equal((await theme()).palette,palette);
            assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${palette} ${width} ${route}`);
            await require('./ui-contract.cjs')(page);
          }
          if ([390,1440].includes(width)) { await navigate('/admin/settings/storefront'); await page.screenshot({path:path.join(output,`admin-theme-${palette}-${name}-${width}.png`),fullPage:true}); }
        }
        await page.setViewportSize(viewport);
      }
    }
    for (const key of ['form','button','cell','table','main','sidebar','card']) assert.ok(densities.compact[key]<densities.comfortable[key],`${key}: actual density difference ${JSON.stringify(densities)}`);
    await navigate('/admin/settings/storefront');
    await page.getByLabel('Colori',{exact:true}).selectOption('forest');
    await page.getByLabel('Caratteri',{exact:true}).selectOption('editorial');
    await page.getByLabel('Densità',{exact:true}).selectOption('comfortable');
    await page.getByLabel('Indirizzo pubblico del negozio',{exact:true}).fill('invalid');
    const invalid=page.waitForResponse(r=>new URL(r.url()).pathname==='/admin/settings/save' && r.request().method()==='POST');
    await page.getByRole('button',{name:'Salva impostazioni',exact:true}).click(); assert.equal((await invalid).status(),422);
    assert.equal((await theme()).palette,'clay'); assert.equal(await page.getByLabel('Colori',{exact:true}).inputValue(),'forest');
    assert.deepEqual((await query('SELECT palette,typography,density FROM storefront_settings WHERE id=1')).rows[0],{palette:'clay',typography:'modern',density:'compact'});
    // Native status request and HTMX replacement both inherit the document variables.
    await navigate('/admin/status');
    if (js) { const response=page.waitForResponse(r=>new URL(r.url()).pathname==='/admin/status'); await page.getByRole('button',{name:'Verifica ora',exact:true}).click(); assert.equal((await response).status(),200); await page.locator('#status-result .ui-message').waitFor(); }
    else { await page.getByRole('button',{name:'Verifica ora',exact:true}).click(); await page.waitForURL(url=>url.pathname==='/admin/status'); }
    assert.equal((await theme()).palette,'clay');
  } finally {
    await query('UPDATE storefront_settings SET palette=$1,typography=$2,density=$3,public_origin=$4 WHERE id=1',[original.palette,original.typography,original.density,original.public_origin]);
    await page.setViewportSize(viewport); await navigate('/admin');
  }
};
