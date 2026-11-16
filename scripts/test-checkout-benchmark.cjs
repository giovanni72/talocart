const { test } = require('node:test');
const assert = require('node:assert/strict');
const { dedicatedDatabase, stats, confirmationFields, technicalFailures } = require('./checkout-benchmark-contracts.cjs');
test('confirmation parser uses the actual checkout form and rejects missing or encoded capabilities', () => {
  const form = values => `<form id="checkout-methods">${Object.entries(values).map(([k,v])=>`<input type="hidden" name="${k}" value="${v}">`).join('')}</form>`;
  const values = {csrf:'ab'.repeat(32),revision:'12',review:'cd'.repeat(32)};
  assert.deepEqual(confirmationFields('<form><input name="csrf" value="wrong"></form>'+form(values)), values);
  assert.throws(()=>confirmationFields(form({...values,review:''})));
  assert.throws(()=>confirmationFields(form({...values,csrf:'&quot;'})));
});
test('timing distributions retain tails and distinguish contention from technical failures', () => {
  assert.equal(stats(Array.from({length:100},(_,n)=>n+1)).p99_ms,99);
  assert.equal(stats([9,1,2]).p50_ms,2);
  assert.throws(()=>stats([]));assert.throws(()=>stats([NaN]));
  assert.equal(technicalFailures([303,409,500,503]),2);
  assert.equal(technicalFailures([422]),1);
  assert.equal(technicalFailures([303,409,422],[409,422]),0);
});

test('mutating benchmark refuses application, remote and preselected-schema databases', () => {
  assert.equal(dedicatedDatabase('postgres://synthetic@127.0.0.1:55432/talocart_test').pathname,'/talocart_test');
  for (const input of ['postgres://synthetic@127.0.0.1/shop','postgres://synthetic@remote.example/shop_test','postgres://synthetic@localhost/shop_test?options=-csearch_path=public']) {
    assert.throws(()=>dedicatedDatabase(input));
  }
});
