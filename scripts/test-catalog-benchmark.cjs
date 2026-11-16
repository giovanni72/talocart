const test = require('node:test');
const assert = require('node:assert/strict');
const { compareReports } = require('./catalog-benchmark-sql.cjs');
const names = ['catalog_it_21','catalog_en_84'];
test('diagnostic SQL decodes continued Rust literals without inserting token whitespace',()=>{
  const {constant}=require('./catalog-benchmark-sql.cjs');
  const rust=String.raw`const QUERY: &str = "SELECT p.id,\
    p.name FROM items p \
    WHERE p.label=\"quoted\"";`;
  assert.equal(constant('QUERY',rust),'SELECT p.id,p.name FROM items p WHERE p.label="quoted"');
  assert.throws(()=>constant('MISSING',rust),/Missing catalog SQL constant/);
  assert.throws(()=>constant('QUERY|MISSING',rust),/Invalid catalog SQL constant name/);
});
test('diagnostic SQL reads every current public listing constant from its owner',()=>{
  const {constant,expand}=require('./catalog-benchmark-sql.cjs');
  const sql=expand(`${constant('PUBLIC_CATEGORY_SCOPE')} ${constant('PUBLIC_CARD_SELECT')} WHERE ${constant('PUBLIC_FILTER')}`,'en','it');
  assert.match(sql,/^WITH RECURSIVE category_scope/);
  assert.ok(!sql.includes('\\\n'));
  assert.ok(!sql.includes('[['));
});
test('count and selector diagnostics resolve continued literals from their current owner modules',()=>{
  const {query,constant,expand}=require('./catalog-benchmark-sql.cjs');
  for(const marker of ['SELECT count(*) FROM [[visible_products]] p JOIN [[products]] source',
    'SELECT name,slug FROM [[brands]] WHERE active','{PUBLIC_CATEGORY_SCOPE}, relevant AS',
    'WITH RECURSIVE relevant_categories(id,parent_id)']){
    const sql=expand(query(marker).replaceAll('{PUBLIC_CATEGORY_SCOPE}',constant('PUBLIC_CATEGORY_SCOPE')).replaceAll('{PUBLIC_FILTER}',constant('PUBLIC_FILTER')),'en','it');
    assert.ok(!sql.includes('[['));assert.ok(!sql.includes('{PUBLIC_'));
  }
  assert.throws(()=>query('SELECT missing','let q = "SELECT other";'),/Expected one catalog owner query/);
  assert.throws(()=>query('SELECT repeated','let a = "SELECT repeated"; let b = "SELECT repeated";'),/Expected one catalog owner query/);
});
const report = time => ({dataset:{products:100,published:99},reads:Object.fromEntries(names.map(name=>[name,{html_sha256:'hash-'+name,p50_ms:time}]))});
test('baseline comparison allows different timings with identical content in every case',()=>{
  const compared=compareReports(report(1000),report(10),names);
  assert.equal(Object.keys(compared).length,2);
  assert.deepEqual(compared.catalog_en_84,{before_p50_ms:1000,after_p50_ms:10,identical_html:true});
});
test('baseline comparison rejects a different dataset',()=>{
  const other=report(10);other.dataset.published=98;
  assert.throws(()=>compareReports(report(1000),other,names),/dataset must match/);
});
test('baseline comparison rejects changed HTML even when the response is faster',()=>{
  const other=report(10);other.reads.catalog_en_84.html_sha256='different';
  assert.throws(()=>compareReports(report(1000),other,names),/HTML differs/);
});
test('baseline comparison rejects missing cases and legacy reports without hashes',()=>{
  const missing=report(10);delete missing.reads.catalog_en_84;
  assert.throws(()=>compareReports(missing,report(10),names),/needs an HTML hash/);
  assert.throws(()=>compareReports(report(10),missing,names),/needs an HTML hash/);
  const legacy=report(10);delete legacy.reads.catalog_it_21.html_sha256;
  assert.throws(()=>compareReports(legacy,report(10),names),/needs an HTML hash/);
});
test('diagnostic SQL uses one translation join per entity for the primary language',()=>{
  const {expand}=require('./catalog-benchmark-sql.cjs');
  for(const locale of ['it','en']){
    const same=expand('SELECT * FROM [[visible_products]]',locale,locale);
    assert.equal((same.match(/_translations /g)||[]).length,3);
    assert.ok(same.includes("coalesce(nullif(cct.name,''),original.category_name)"));
    assert.ok(!same.includes('ccp.'));
    const secondary=expand('SELECT * FROM [[visible_products]]',locale,locale==='it'?'en':'it');
    assert.equal((secondary.match(/_translations /g)||[]).length,6);
    assert.ok(secondary.includes("nullif(ccp.name,'')"));
  }
});
