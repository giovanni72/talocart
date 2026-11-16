// Diagnostic expansion of the owner's explicit projections, including localization.
// Read columns/fields from Rust so changes fail visibly instead of measuring stale SQL.
const fs = require('node:fs');
const assert = require('node:assert/strict');
const source = fs.readFileSync('crates/commerce/src/products.rs', 'utf8');
const content = fs.readFileSync('crates/commerce/src/content.rs', 'utf8');
const readers = ['reads', 'commands'].map(name => fs.readFileSync(`crates/commerce/src/products/${name}.rs`, 'utf8')).join('\n');
const decodeLiteral = value => JSON.parse('"' + value.replace(/\\\r?\n\s*/g, '').replace(/\r?\n/g, '\\n') + '"');
const constant = (name, input = source) => {
  if (!/^[A-Z][A-Z0-9_]*$/.test(name)) throw new Error('Invalid catalog SQL constant name');
  const value = input.match(new RegExp(String.raw`\bconst ${name}: &str = "((?:[^"\\]|\\[\s\S])*)";`));
  if (!value) throw new Error(`Missing catalog SQL constant ${name}`);
  // Decode Rust continuations/quoted escapes as data, preserving token boundaries.
  return decodeLiteral(value[1]);
};
function query(marker, input = readers) {
  const normalize = value => value.replace(/\s+/g, ' ');
  const matches = [];
  for (const value of input.matchAll(/"((?:[^"\\]|\\[\s\S])*)"/g)) {
    let sql;
    // Non-SQL Rust strings can use escapes outside the quoted SQL subset.
    try { sql = decodeLiteral(value[1]); } catch { continue; }
    if (normalize(sql).includes(normalize(marker))) matches.push(sql);
  }
  assert.equal(matches.length, 1, 'Expected one catalog owner query');
  return matches[0];
}
function expand(sql, current, primary) {
  if (![current, primary].every(locale => ['it', 'en'].includes(locale))) throw new Error('Unsupported benchmark locale');
  for (const [, table, columns, fields] of content.matchAll(/\(\s*"(catalog_[^"]+)",\s*"([^"]+)",\s*vec!\[([^\]]*)\]/g)) {
    const token = '[[' + table.replace('catalog_', '') + ']]';
    if (!sql.includes(token)) continue;
    const visible = table === 'catalog_visible_products';
    const base = visible ? 'catalog_products' : table;
    const expression = (field, original, t='ct', p='cp') => current === primary
      ? `coalesce(nullif(${t}.${field},''),${original})`
      : `coalesce(nullif(${t}.${field},''),nullif(${p}.${field},''),${original})`;
    const translatedJoins = (table, id, t='ct', p='cp') => ` LEFT JOIN ${table}_translations ${t} ON ${t}.entity_id=${id} AND ${t}.locale='${current}'`
      + (current === primary ? '' : ` LEFT JOIN ${table}_translations ${p} ON ${p}.entity_id=${id} AND ${p}.locale='${primary}'`);
    let joins = translatedJoins(base, 'original.id');
    const projection = columns.split(',').map(column => {
      if (fields.includes('"' + column + '"')) return `${expression(column, 'original.' + column)} AS ${column}`;
      if (visible && ['category_name', 'brand_name'].includes(column)) {
        const prefix = column === 'category_name' ? 'cc' : 'bc';
        return `${expression('name', 'original.' + column, prefix+'t', prefix+'p')} AS ${column}`;
      }
      return 'original.' + column;
    }).join(',');
    if (visible) for (const [prefix, owner, id] of [['cc', 'categories', 'category_id'], ['bc', 'brands', 'brand_id']]) {
      joins += translatedJoins('catalog_' + owner, 'original.' + id, prefix+'t', prefix+'p');
    }
    sql = sql.split(token).join(`(SELECT ${projection} FROM ${table} original ${joins})`);
  }
  if (sql.includes('[[')) throw new Error('Unexpanded catalog projection');
  return sql;
}
function compareReports(baseline, current, names) {
  assert.deepEqual(baseline.dataset,current.dataset,'Baseline dataset must match');
  const comparison={};
  for (const name of names) {
    const before=baseline.reads[name],after=current.reads[name];
    assert.ok(before?.html_sha256,`${name}: baseline needs an HTML hash`);
    assert.ok(after?.html_sha256,`${name}: current report needs an HTML hash`);
    assert.equal(after.html_sha256,before.html_sha256,`${name}: baseline HTML differs`);
    comparison[name]={before_p50_ms:before.p50_ms,after_p50_ms:after.p50_ms,identical_html:true};
  }
  return comparison;
}
module.exports = { source, constant, query, expand, compareReports };
