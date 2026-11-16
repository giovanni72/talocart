(() => {
 const form=document.querySelector('#checkout-details');if(!form)return;
 const get=id=>document.getElementById(id);
 const group=(key,visible)=>{const el=form.querySelector(`[data-co-group="${key}"]`);if(el){el.hidden=!visible;el.disabled=!visible;}};
 const mode=(name,kind)=>form.querySelector(`[name="${name}"]`)?.closest('[data-field-policy]')?.dataset[kind+'Mode']||'hidden';
 const hasFields=(key,kind)=>[...form.querySelectorAll(`[data-co-group="${key}"] [data-field-policy]`)].some(row=>row.dataset[kind+'Mode']!=='hidden');
 const update=()=>{
  const kind=form.querySelector('[name="customer_type"]:checked')?.value||'private',business=kind==='business';
  const invoice=business||get('invoice_requested').checked,same=get('billing_same').checked;
  const country=same?get('shipping_country').value:get('billing_country').value;
  get('invoice-choice').hidden=business;
  group('private',invoice&&hasFields('private',kind));group('business',invoice&&hasFields('business',kind));group('billing',invoice&&!same);
  const code=mode('recipient_code',kind),pec=mode('pec',kind);
  const italianInvoice=invoice&&country==='IT';
  group('fiscal',italianInvoice&&hasFields('fiscal',kind));
  group('code',italianInvoice&&code!=='hidden');group('pec',italianInvoice&&pec!=='hidden');
  form.querySelectorAll('[data-field-policy]').forEach(row=>{
   const state=row.dataset[kind+'Mode'];row.hidden=state==='hidden';
   const name=row.querySelector('[name]')?.name||'';
   const billing=name.startsWith('billing_')||['vat_id','tax_id','recipient_code','pec'].includes(name);
   const destination=billing?country:get('shipping_country').value;
   let required=state==='required'&&(!billing||invoice);
   if(name.endsWith('_region'))required&&=['IT','US','CA','AU'].includes(destination);
   if(name.endsWith('_postal'))required&&=['IT','DE','FR','ES','AT','CH','AU','US','CA','GB'].includes(destination);
   if(['tax_id','recipient_code','pec'].includes(name))required&&=italianInvoice;
   row.querySelectorAll('input,select,textarea').forEach(input=>{input.disabled=row.hidden;input.required=required;});
   const mark=row.querySelector('[data-required-mark]');if(mark)mark.hidden=!required;
  });
 };
 form.addEventListener('change',update);update();
 for(const suffix of ['first_name','last_name'])get(suffix)?.addEventListener('blur',()=>{for(const prefix of ['recipient_','billing_'])if(get(prefix+suffix)&&!get(prefix+suffix).value)get(prefix+suffix).value=get(suffix).value;});
 form.querySelector('[aria-invalid="true"]')?.focus();
 // Refresh only the quotation after a minicart mutation; never replace the draft form.
 const status=get('checkout-cart-update'),message=status.querySelector('[role=status]'),retry=status.querySelector('button');
 let generation=0,controller=null,updating=false;
 const disabled=new Set();
 const lock=()=>{
  updating=true;status.hidden=false;retry.hidden=true;message.textContent=status.dataset.loading;
  for(const button of form.querySelectorAll('button[type=submit]'))if(!button.disabled){disabled.add(button);button.disabled=true;}
 };
 document.addEventListener('minicart:changing',()=>{generation++;controller?.abort();lock();});
 const refresh=async()=>{
  const current=++generation;controller?.abort();controller=new AbortController();const request=controller;
  lock();
  const timeout=setTimeout(()=>request.abort(),15000);
  try{
   const body=new URLSearchParams(new FormData(form));body.set('action','refresh');
   const response=await fetch(form.getAttribute('action'),{method:'POST',credentials:'same-origin',body,signal:request.signal});
   if(!response.ok||response.redirected)throw new Error('Checkout refresh unavailable');
   const parsed=new DOMParser().parseFromString(await response.text(),'text/html');
   const summary=parsed.getElementById('checkout-summary'),revision=parsed.querySelector('#checkout-details [name=revision]');
   if(!summary||!revision)throw new Error('Invalid checkout refresh');
   if(current!==generation)return;
   get('checkout-summary').replaceWith(summary);form.elements.revision.value=revision.value;
   updating=false;for(const button of disabled)button.disabled=false;disabled.clear();status.hidden=true;
  }catch{
   if(current!==generation)return;
   message.textContent=status.dataset.error;retry.hidden=false;
  }finally{clearTimeout(timeout);}
 };
 document.addEventListener('minicart:changed',()=>{void refresh();});
 retry.addEventListener('click',()=>{void refresh();});
 form.addEventListener('submit',event=>{if(updating)event.preventDefault();});

})();
