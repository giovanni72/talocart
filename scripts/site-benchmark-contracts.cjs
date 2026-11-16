const assert = require('node:assert/strict');
// Per-session CSRF and MVCC settings revisions change between disposable schemas.
// Preserve business revisions, names, prices, quantities, links and row order.
function normalizeHtml(html, route="") {
  if(route==="/admin/settings")html=html.replace(/((?:Anteprima della configurazione salvata|Saved configuration preview): <output>)[^<]*(<\/output>)/g,"$1TIME$2");
  if(route==="/admin/settings/email")html=html.replace(/(<input\b[^>]*\btype="hidden"[^>]*\bname="key"[^>]*\bvalue=")[a-f0-9]{64}/g,'$1IDEMPOTENCY_TOKEN');
  return html.replace(/(<input\b[^>]*\bname="(?:csrf|customer_csrf|cart_csrf|settings_revision)"[^>]*\bvalue=")[^"]*/g,'$1TOKEN');
}
function compareReports(before, after) {
  assert.deepEqual(after.dataset,before.dataset,'Benchmark dataset must match');
  const key=row=>row.name+' '+row.route;
  assert.deepEqual(after.reads.map(key),before.reads.map(key),'Scenario inventory must match');
  return after.reads.map((row,i)=>{
    const old=before.reads[i];
    assert.ok(old.html_sha256,`${key(row)}: missing baseline HTML hash`);
    assert.equal(row.html_sha256,old.html_sha256,`${key(row)}: normalized HTML differs`);
    return {scenario:key(row),before_ms:old.ttfb.p50_ms,after_ms:row.ttfb.p50_ms,identical_html:true};
  });
}
module.exports={normalizeHtml,compareReports};
// Stable query-shape gates, independent of machine timing and background workers.
function verifyQueryBudgets(report) {
  const dashboard=report.diagnostics.find(x=>x.route==='/admin');
  const edit=report.diagnostics.find(x=>x.route==='/admin/catalog/products/1');
  assert.ok(dashboard && edit,'Missing query diagnostics');
  assert.ok(dashboard.summaries.filter(x=>x.summary?.startsWith('SELECT s.token_hash,s.csrf,s.administrator_id')).length<=3,'Dashboard repeats authorization reads');
  verifyProductEditorQueryBudget(edit);
}
function verifyProductEditorQueryBudget(edit) {
  assert.ok(edit,'Missing product editor query diagnostics');
  assert.equal(edit.summaries.filter(x=>x.summary?.startsWith('SELECT p.*,EXISTS(')).length,1,'Product edit must read its entry once');
  assert.equal(edit.summaries.filter(x=>x.summary?.startsWith('SELECT EXISTS(SELECT 1 FROM')).length,2,'Product editor checks content existence and public visibility without loading a public entry');
  assert.equal(edit.summaries.filter(x=>x.summary?.startsWith('SELECT count(*) FROM catalog_variants')).length,0,'Product editor must reuse its loaded variant count');
  assert.equal(edit.summaries.filter(x=>x.summary?.startsWith('SELECT p.*,source.seo_title')).length,0,'Public link needs only a visibility check');
  assert.ok(edit.summaries.filter(x=>x.summary?.startsWith('SELECT s.token_hash,s.csrf,s.administrator_id')).length<=8,'Product attributes must reuse the product authorization');
}
module.exports.verifyQueryBudgets=verifyQueryBudgets;
module.exports.verifyProductEditorQueryBudget=verifyProductEditorQueryBudget;
function verifyMediaPlan(plan) {
  const scans=[];
  const visit=node=>{
    if(node['Relation Name']==='catalog_variants')scans.push(node);
    for(const child of node.Plans||[])visit(child);
  };
  visit(plan.Plan);
  assert.ok(scans.length,'Media usage plan must include the variant references');
  assert.ok(scans.every(node=>node['Actual Loops']<=1),'Media usage must not rescan variants per library source');
}
module.exports.verifyMediaPlan=verifyMediaPlan;
// The focused fixture has 1,000 variants for this product and 5,450 globally.
function verifyProductVariantScan(plan, productVariants) {
  const scans=[];
  const visit=node=>{if(node['Relation Name']==='catalog_variants')scans.push(node);for(const child of node.Plans||[])visit(child);};
  visit(plan.Plan);
  assert.ok(scans.length,'Missing product variant scan');
  assert.ok(scans.every(node=>node['Actual Rows']+(node['Rows Removed by Filter']||0)<=productVariants),'Option selection must restrict variant work to the current product');
}
module.exports.verifyProductVariantScan=verifyProductVariantScan;
function requestQueries(logs, requestId) {
  return logs.filter(row=>row.target==='sqlx::query' && (row.span?.request_id===requestId || row.spans?.some(span=>span.request_id===requestId)));
}
module.exports.requestQueries=requestQueries;
