const assert = require('node:assert/strict');
function dedicatedDatabase(input) {
  const url = new URL(input);
  assert.ok(['localhost','127.0.0.1','[::1]'].includes(url.hostname) && url.pathname.endsWith('_test') && !url.search,
    'Dedicated loopback _test database without connection options required');
  return url;
}
function stats(values) {
  assert.ok(values.length && values.every(x => Number.isFinite(x) && x >= 0), 'Nonempty finite timings required');
  const sorted = [...values].sort((a, b) => a - b);
  const at = p => +sorted[Math.ceil(sorted.length * p) - 1].toFixed(3);
  return { samples: sorted.length, p50_ms: at(.5), p95_ms: at(.95), p99_ms: at(.99), max_ms: at(1) };
}
function confirmationFields(html) {
  const form = html.match(/<form\b[^>]*\bid="checkout-methods"[^>]*>([\s\S]*?)<\/form>/);
  assert.ok(form, 'Real methods form required');
  const fields = {};
  for (const name of ['csrf', 'revision', 'review']) {
    const value = form[1].match(new RegExp(`<input\\b[^>]*name="${name}"[^>]*value="([^"]*)"`))?.[1];
    assert.ok(value && /^[a-f0-9]+$/.test(value), `Missing or unsafe ${name}`);
    fields[name] = value;
  }
  return fields;
}
function technicalFailures(statuses, expectedBusiness = [409]) { return statuses.filter(x => x !== 303 && !expectedBusiness.includes(x)).length; }
module.exports = { dedicatedDatabase, stats, confirmationFields, technicalFailures };
