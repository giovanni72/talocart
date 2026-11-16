// Frozen default-theme reference CSS is a visual regression oracle, never a runtime asset.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const reference = fs.readFileSync(path.join(__dirname, 'fixtures/default-theme-reference.css'), 'utf8');
const routed=new WeakMap();
async function verify(page, name) {
  // The second theme intentionally has a different presentation.
  if (await page.locator('body[data-theme="essential"]').count()) return;
  if(!routed.has(page)){
    const state={requests:0,failures:[]};routed.set(page,state);
    page.on('requestfailed',r=>{if(r.url().includes('__theme_baseline'))state.failures.push(r.failure());});
    await page.route('**/__theme_baseline.css',async route=>{state.requests++;await route.fulfill({contentType:'text/css; charset=utf-8',body:reference});});
  }
  await page.evaluate(()=>{
    if(document.getElementById('theme-regression-baseline'))return;
    const link=document.createElement('link');link.id='theme-regression-baseline';link.rel='stylesheet';link.media='all';link.href='/__theme_baseline.css';
    document.head.insertBefore(link,document.querySelector('link[href="/assets/base.css"]'));
  });
  // No-JS documents do not run event/timer callbacks. Poll from Node, not from
  // waitForFunction's in-page timer: the native stylesheet can load meanwhile.
  const deadline=Date.now()+10000;let loaded=false;
  while(Date.now()<deadline){
    if(await page.evaluate(()=>Boolean(document.getElementById('theme-regression-baseline')?.sheet))){loaded=true;break;}
    await new Promise(resolve=>setTimeout(resolve,25));
  }
  if(!loaded){const state=await page.evaluate(()=>({path:location.pathname,ready:document.readyState,link:document.getElementById('theme-regression-baseline')?.outerHTML,sheets:Array.from(document.styleSheets,s=>s.href)}));throw Error(JSON.stringify({name,...state,network:routed.get(page),cause:'Baseline stylesheet did not load'}));}
  await page.evaluate(()=>{document.getElementById('theme-regression-baseline').media='not all';});
  await page.evaluate(() => document.fonts.ready);
  const differences = await page.evaluate(() => {
    const base = document.querySelector('link[href="/assets/base.css"]')?.sheet;
    if (!base) throw Error('Missing shared stylesheet');
    const theme = document.querySelector('link[href="/assets/themes/default.css"]')?.sheet;
    const baseline=document.getElementById('theme-regression-baseline');
    const take = () => Array.from(document.querySelectorAll('body, body *')).filter(e => !['SCRIPT','STYLE'].includes(e.tagName)).map(e => {
      const values = {};
      const visible=e.checkVisibility({visibilityProperty:true,opacityProperty:true});
      for (const pseudo of [null, '::before', '::after']) {
        if (!visible) continue;
        const style = getComputedStyle(e, pseudo);
        values[pseudo || 'element'] = Array.from(style, p => [p, style.getPropertyValue(p)]);
      }
      const r = e.getBoundingClientRect();
      return {node:e.tagName+'#'+e.id+'.'+e.className, visible, rect:visible?[r.x,r.y,r.width,r.height]:null, values};
    });
    const before = take(); let after;
    try {
      base.disabled=true;if(theme)theme.disabled=true;baseline.media='all';
      after=take();
    } finally {baseline.media='not all';base.disabled=false;if(theme)theme.disabled=false;}
    const changed = [];
    for (let i=0;i<before.length;i++) {
      const a=before[i],b=after[i];
      if(a.visible!==b.visible){changed.push({node:a.node,visible:a.visible,previous:b.visible});continue;}
      if (JSON.stringify(a.rect)!==JSON.stringify(b.rect)) changed.push({node:a.node,rect:a.rect,previous:b.rect});
      for (const pseudo of Object.keys(a.values)) {
        const previous=new Map(b.values[pseudo]);
        for (const [property,value] of a.values[pseudo]) if(value!==previous.get(property)) changed.push({node:a.node,pseudo,property,value,previous:previous.get(property)});
      }
      if(changed.length>8)break;
    }
    return changed.slice(0,10);
  });
  assert.deepEqual(differences, [], `Presentation changed: ${name}`);
  const out=path.resolve('.local/verification/theme-boundary');fs.mkdirSync(out,{recursive:true});
  fs.appendFileSync(path.join(out,'comparisons.jsonl'),JSON.stringify({name,pathname:new URL(page.url()).pathname,passed:true})+'\n');
}
module.exports = { verify };
