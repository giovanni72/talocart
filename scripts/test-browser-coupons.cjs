const assert=require('node:assert/strict');
const http=require('node:http');
const {once}=require('node:events');

async function verifyCouponNavigation(browser,applyCoupon){
 let release,redirected;
 const reached=new Promise(resolve=>redirected=resolve);
 const held=new Promise(resolve=>release=resolve);
 let gets=0;
 const server=http.createServer(async(req,res)=>{
  if(req.url==='/favicon.ico'){res.writeHead(204);res.end();return;}
  if(req.method==='POST'){
   req.resume();await once(req,'end');
   res.writeHead(303,{Location:'/cart?saved=1'});res.end();return;
  }
  if(++gets===2){redirected();await held;}
  res.writeHead(200,{'Content-Type':'text/html'});
  res.end('<!doctype html><html><body><form method="post" action="/cart/coupon"><input id="coupon-code" name="code"><div><button name="action" value="apply">Apply</button></div></form><p id="document">'+gets+'</p></body></html>');
 });
 const context=await browser.newContext({viewport:{width:390,height:1000},javaScriptEnabled:false});
 try{
  server.listen(0,'127.0.0.1');await once(server,'listening');
  const page=await context.newPage();
  // Reapplying a coupon starts and ends on the same URL. URL equality is no barrier.
  await page.goto('http://127.0.0.1:'+server.address().port+'/cart?saved=1');
  let completed=false;
  const submission=applyCoupon(page,'SAVE10').then(()=>{completed=true;});
  try{
   // Propagate submission errors as well; a missing POST must not strand this fixture.
   await Promise.race([reached,submission]);
   // Let the already-received 303 propagate through Playwright's client callbacks.
   // The redirected GET remains held by the server; no timing delay releases it.
   await context.cookies();
   assert.equal(completed,false,'The previous document being loaded cannot complete a new submission');
  }finally{release();await submission;}
  assert.equal(await page.locator('#document').innerText(),'2');
 }finally{
  release();await context.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
 }
}
module.exports={verifyCouponNavigation};
if(require.main===module){
 const {test}=require('node:test');
 const {chromium}=require('playwright');
 const {applyCoupon}=require('./browser-coupons.cjs');
 test('coupon keyboard submission waits for the redirected document, even at the same URL',async()=>{
  const browser=await chromium.launch(process.env.TALOCART_BROWSER_CHANNEL?{channel:process.env.TALOCART_BROWSER_CHANNEL}:{});
  try{await verifyCouponNavigation(browser,applyCoupon);}finally{await browser.close();}
 });
}
