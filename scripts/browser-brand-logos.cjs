// Uses browser-demo's isolated schema/media; never changes application data.
const assert = require('node:assert/strict');
const path = require('node:path');
module.exports = async ({page,query,origin,output,name,viewport}) => {
  const settings=(await query('SELECT theme_id FROM storefront_settings WHERE id=1')).rows[0];
  const media=(await query('SELECT key FROM media_objects WHERE public ORDER BY id LIMIT 1')).rows[0];
  assert.ok(media,'Demo provides a public image for the logo fixture');
  const brand=(await query("INSERT INTO catalog_brands(name,slug,logo_key) VALUES ('A logo test','menu-logo-test',$1) RETURNING id",[media.key])).rows[0].id;
  await query("INSERT INTO catalog_brands(name,slug) VALUES ('A without logo','menu-logo-absent')");
  for (const [locale,label] of [['it','A Marchio <logo>'],['en','A Brand <logo>']]) {
    await query('INSERT INTO catalog_brands_translations(entity_id,locale,name,description) VALUES ($1,$2,$3,\'\') ON CONFLICT(entity_id,locale) DO UPDATE SET name=excluded.name',[brand,locale,label]);
  }
  try {
    for(const theme of ['default','essential']) {
      await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[theme]);
      for(const [prefix,locale,label] of [['','it','A Marchio <logo>'],['/en','en','A Brand <logo>']]) {
        for(const width of [320,390,1440]) {
          await page.setViewportSize({width,height:1050});
          await page.goto(origin+prefix+'/brands');
          assert.equal(await page.locator('html').getAttribute('lang'),locale);
          const navigation=page.locator('.category-navigation'),dropdown=page.locator('.brand-dropdown');
          if(!await navigation.evaluate(e=>e.open)) await navigation.locator(':scope > summary').click();
          await dropdown.locator(':scope > summary').focus();await page.keyboard.press('Enter');
          for(const container of [dropdown.locator('.brand-panel'),page.locator('.brand-directory-list')]) {
            const directory=await container.evaluate(e=>e.classList.contains('brand-directory-list'));
            const link=container.locator(`a[href="${prefix}/brands/menu-logo-test"]`),image=link.locator('.brand-link-logo');
            assert.equal(await link.innerText(),label);
            assert.equal(await image.getAttribute('alt'),'');
            await link.focus();
            await image.evaluate(e=>e.decode());
            const geometry=await link.evaluate(e=>{
              const img=e.querySelector('img'),name=e.querySelector('.brand-link-name'),r=img.getBoundingClientRect(),n=name.getBoundingClientRect(),slot=img.parentElement,s=slot.getBoundingClientRect(),style=getComputedStyle(slot);
              return {loaded:img.complete&&img.naturalWidth>0,width:r.width,height:r.height,before:r.right<=n.left,above:r.bottom<=n.top,
                slotWidth:s.width,slotHeight:s.height,innerWidth:slot.clientWidth-parseFloat(style.paddingLeft)-parseFloat(style.paddingRight),innerHeight:slot.clientHeight-parseFloat(style.paddingTop)-parseFloat(style.paddingBottom),
                fit:getComputedStyle(img).objectFit,focus:e===document.activeElement&&getComputedStyle(e).outlineStyle!=='none'};
            });
            assert.ok(geometry.loaded&&(directory?geometry.above:geometry.before)&&geometry.focus,JSON.stringify(geometry));
            assert.ok(Math.abs(geometry.width-geometry.innerWidth)<1&&Math.abs(geometry.height-geometry.innerHeight)<1,'Logo fills its padded slot without cropping');
            assert.equal(geometry.slotHeight,directory?(width<=800?104:128):44,'Responsive cards and compact menu retain their media slots');
            if(!directory)assert.equal(geometry.slotWidth,76,'Menu logo slot precedes the name');
            if(directory) {
              const without=container.locator(`a[href="${prefix}/brands/menu-logo-absent"]`);
              assert.equal((await without.boundingBox()).height,(await link.boundingBox()).height,'Cards keep the same height with or without a logo');
            }
            assert.equal(geometry.fit,'contain');
            assert.equal(await container.locator(`a[href="${prefix}/brands/menu-logo-absent"] img`).count(),0);
            assert.equal(await container.locator(`a[href="${prefix}/brands/menu-logo-absent"] .brand-initial`).count(),1);
          }
          assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
          await dropdown.locator(`a[href="${prefix}/brands/menu-logo-test"]`).focus();
          await page.screenshot({path:path.join(output,`brand-logos-${name}-${theme}-${locale}-${width}.png`)});
          await page.keyboard.press('Enter');
          await page.waitForURL(origin+prefix+'/brands/menu-logo-test');
          assert.equal(await page.locator('.brand-dropdown a[aria-current]').getAttribute('href'),prefix+'/brands/menu-logo-test');
        }
      }
    }
  } finally {
    await query('UPDATE storefront_settings SET theme_id=$1 WHERE id=1',[settings.theme_id]);
    await query("DELETE FROM catalog_brands WHERE slug IN ('menu-logo-test','menu-logo-absent')");
    await page.setViewportSize(viewport);await page.goto(origin+'/');
  }
};
