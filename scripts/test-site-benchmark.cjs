const {test}=require('node:test');
const assert=require('node:assert/strict');
const {normalizeHtml,compareReports}=require('./site-benchmark-contracts.cjs');
test('comparison ignores only volatile form tokens and preserves business values',()=>{
 const a='<input name="csrf" value="old"><input name="settings_revision" value="old"><input name="revision" value="7"><p>EUR 10,00</p>';
 assert.equal(normalizeHtml(a),normalizeHtml(a.replaceAll('old','new')));
 assert.notEqual(normalizeHtml(a),normalizeHtml(a.replace('10,00','11,00')));
 const key=n=>`<input type="hidden" name="key" value="${n.repeat(64)}">`;
 assert.equal(normalizeHtml(key('a'),'/admin/settings/email'),normalizeHtml(key('b'),'/admin/settings/email'));
 assert.notEqual(normalizeHtml(key('a'),'/admin/roles'),normalizeHtml(key('b'),'/admin/roles'));
 assert.notEqual(normalizeHtml('<input name="fiscal_revision" value="1">'),normalizeHtml('<input name="fiscal_revision" value="2">'));
 assert.equal(normalizeHtml('Saved configuration preview: <output>10:00</output>','/admin/settings'),normalizeHtml('Saved configuration preview: <output>11:00</output>','/admin/settings'));
 assert.notEqual(normalizeHtml('<output>10</output>','/cart'),normalizeHtml('<output>11</output>','/cart'));
 assert.notEqual(normalizeHtml(a),normalizeHtml(a.replace('value="7"','value="8"')));
});
test('comparison rejects changed results and missing scenarios',()=>{
 const baseline={dataset:{products:100},reads:[{name:'it',route:'/cart',html_sha256:'one',ttfb:{p50_ms:10}}]};
 assert.equal(compareReports(baseline,baseline)[0].identical_html,true);
 assert.throws(()=>compareReports(baseline,{...baseline,reads:[]}));
 assert.throws(()=>compareReports(baseline,{...baseline,reads:[{...baseline.reads[0],html_sha256:'two'}]}));
});
test('media plan rejects per-source rescans independently of wall-clock timing',()=>{
 const {verifyMediaPlan}=require('./site-benchmark-contracts.cjs');
 const plan=loops=>({Plan:{Plans:[{'Relation Name':'catalog_variants','Actual Loops':loops}]}});
 assert.doesNotThrow(()=>verifyMediaPlan(plan(1)));
 assert.throws(()=>verifyMediaPlan(plan(1372)));
 assert.throws(()=>verifyMediaPlan({Plan:{Plans:[]}}));
});
test('product option plan rejects scans of other products variants',()=>{
 const {verifyProductVariantScan}=require('./site-benchmark-contracts.cjs');
 const plan=(rows,removed=0)=>({Plan:{Plans:[{'Relation Name':'catalog_variants','Actual Rows':rows,'Rows Removed by Filter':removed}]}});
 assert.doesNotThrow(()=>verifyProductVariantScan(plan(1000),1000));
 assert.throws(()=>verifyProductVariantScan(plan(5450),1000));
 assert.throws(()=>verifyProductVariantScan(plan(1000,4450),1000));
 assert.throws(()=>verifyProductVariantScan({Plan:{}},1000));
});
test('query counts correlate the request and exclude background maintenance',()=>{
 const {requestQueries}=require('./site-benchmark-contracts.cjs');
 const own={target:'sqlx::query',span:{request_id:'one'}};
 const nested={target:'sqlx::query',spans:[{request_id:'one'},{name:'nested'}]};
 assert.deepEqual(requestQueries([own,nested,{target:'sqlx::query'},{target:'sqlx::query',span:{request_id:'other'}},{target:'http',span:{request_id:'one'}}],'one'),[own,nested]);
});

test('product editor query budget rejects repeated existence and permission reads',()=>{
 const {verifyQueryBudgets}=require('./site-benchmark-contracts.cjs');
 const auth={summary:'SELECT s.token_hash,s.csrf,s.administrator_id'};
 const exists={summary:'SELECT EXISTS(SELECT 1 FROM'};
 const summaries=[{summary:'SELECT p.*,EXISTS('},exists,exists,...Array(8).fill(auth)];
 const report=rows=>({diagnostics:[{route:'/admin',summaries:[auth]},{route:'/admin/catalog/products/1',summaries:rows}]});
 assert.doesNotThrow(()=>verifyQueryBudgets(report(summaries)));
 assert.throws(()=>verifyQueryBudgets(report([...summaries,exists])));
 assert.throws(()=>verifyQueryBudgets(report([...summaries,auth])));
 assert.throws(()=>verifyQueryBudgets(report([...summaries,{summary:'SELECT count(*) FROM catalog_variants WHERE product_id=$1'}])));
 assert.throws(()=>verifyQueryBudgets(report([...summaries,{summary:'SELECT p.*,source.seo_title,source.seo_description'}])));
});
