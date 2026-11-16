const {test}=require('node:test');
const assert=require('node:assert/strict');
const vm=require('node:vm');
const fs=require('node:fs');
const source=fs.readFileSync('assets/payment-status.js','utf8');
const flush=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(fetcher) {
 let now=0,sequence=0;const timers=new Map(),events={},calls=[],results=[],stops=[],busy=[],storage=new Map();
 const context={window:{addEventListener:(name,fn)=>events[name]=fn},document:{hidden:false},sessionStorage:{getItem:k=>storage.get(k),setItem:(k,v)=>storage.set(k,v),removeItem:k=>storage.delete(k)},AbortController,AbortSignal,Date:{now:()=>now},setTimeout:(fn,ms)=>{const id=++sequence;timers.set(id,{fn,at:now+ms});return id;},clearTimeout:id=>timers.delete(id),fetch:async(...args)=>{calls.push(args);return fetcher(...args);}};
 vm.runInNewContext(source,context);
 const api=context.window.TalocartPaymentStatus;
 const watcher=api.watch({url:'/payment-status',onStatus:value=>results.push(value),onStop:reason=>stops.push(reason),onBusy:value=>busy.push(value)});
 return {api,watcher,context,calls,results,stops,busy,events,timers,async advance(ms){now+=ms;for(const [id,t]of [...timers])if(t.at<=now){timers.delete(id);t.fn();}await flush();}};
}
const response=(state='due')=>({ok:true,json:async()=>({state,can_resume:false,receipt_url:state==='paid'?'/receipt':null})});
test('serial JSON requests, explicit refresh and terminal stop',async()=>{
 let release;const f=fixture(()=>new Promise(resolve=>release=resolve));
 f.watcher.start();f.watcher.start();assert.equal(f.calls.length,1);
 assert.equal(f.calls[0][1].headers.Accept,'application/json');assert.equal(f.calls[0][1].cache,'no-store');
 release(response());await flush();assert.equal(f.results.length,1);assert.deepEqual(f.busy,[true,false]);
 f.watcher.stop();await f.advance(2000);assert.equal(f.calls.length,1);
 f.watcher.start();release(response('paid'));await flush();assert.equal(f.results[1].state,'paid');f.watcher.stop();
});
test('three failures stop, manual restart gets a new budget',async()=>{
 const f=fixture(()=>{throw Error('offline')});f.watcher.start();await flush();await f.advance(2000);await f.advance(2000);
 assert.equal(f.calls.length,3);assert.deepEqual(f.stops,['network']);await f.advance(2000);assert.equal(f.calls.length,3);
 f.watcher.start();await flush();assert.equal(f.calls.length,4);f.watcher.stop();
});
test('hidden pages pause automatic requests but manual checks work',async()=>{
 const f=fixture(()=>response());f.context.document.hidden=true;f.watcher.start(false);await f.advance(2000);assert.equal(f.calls.length,0);
 f.watcher.start();await flush();assert.equal(f.calls.length,1);f.watcher.stop();
});
test('one-minute deadline stops until explicitly restarted',async()=>{
 const f=fixture(()=>response());f.watcher.start(false);await f.advance(61000);assert.deepEqual(f.stops,['long']);assert.equal(f.calls.length,0);
 f.watcher.start();await flush();assert.equal(f.calls.length,1);f.watcher.stop();
});
test('pagehide aborts requests and ignores late successful responses',async()=>{
 let release;const f=fixture(()=>new Promise(resolve=>release=resolve));f.watcher.start();f.events.pagehide();
 assert.equal(f.calls[0][1].signal.aborted,true);release(response('paid'));await flush();assert.equal(f.results.length,0);assert.equal(f.timers.size,0);
});
test('invalid states and missing confirmation URLs cannot confirm payment',async()=>{
 for(const payload of [{state:'paid',can_resume:false},{state:'paid',can_resume:false,receipt_url:''},{state:'unknown',can_resume:false}]) {
  const f=fixture(()=>({ok:true,json:async()=>payload}));f.watcher.start();await flush();await f.advance(2000);await f.advance(2000);
  assert.equal(f.results.length,0);assert.deepEqual(f.stops,['network']);
 }
});
test('hints are optional and scoped per order',()=>{
 const f=fixture(()=>response());f.api.pending('a',true);assert.equal(f.api.pending('a'),true);assert.equal(f.api.pending('b'),false);
 f.api.pending('a',false);assert.equal(f.api.pending('a'),false);f.context.sessionStorage.getItem=()=>{throw Error('disabled')};assert.equal(f.api.pending('a'),false);
});
test('receipt updates keep navigation labels out of the document title',async()=>{
 let onStatus,stopped=false;
 const receipt={dataset:{reference:'ORDER-1',paymentState:'due',statusUrl:'/receipt',confirmed:'Confirmed'},contains:()=>false,replaceWith:()=>{}};
 const next={dataset:{reference:'ORDER-1',paymentState:'paid',confirmed:'Confirmed'}};
 const title={textContent:'Order confirmed'},announcement={};
 const document={activeElement:null,getElementById:id=>({'order-receipt':receipt,'receipt-title':title,'receipt-announcement':announcement,'online-payment-refresh-help':{}}[id]),querySelector:()=>({dataset:{storeName:'Shop & Co'},textContent:'Shop & Co Explore Catalogue Your purchases Information'})};
 vm.runInNewContext(fs.readFileSync('assets/order-receipt.js','utf8'),{
  window:{TalocartPaymentStatus:{terminal:s=>s==='paid',pending:()=>false,watch:options=>{onStatus=options.onStatus;return{start:()=>{},stop:()=>{stopped=true;}};}}},
  document,location:{href:'http://127.0.0.1/receipt'},URL,AbortSignal,
  fetch:async()=>({ok:true,text:async()=>'<receipt>'}),DOMParser:class{parseFromString(){return{getElementById:()=>next};}}
 });
 await onStatus({state:'paid'},new AbortController().signal);
 assert.equal(document.title,'Order confirmed ORDER-1 · Shop & Co');
 assert.equal(announcement.textContent,'Confirmed');assert.equal(stopped,true);
});
