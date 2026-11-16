(() => {
 const form=document.querySelector('#customer-form');if(!form)return;
 const type=form.querySelector('[name=customer_type]'),country=form.querySelector('[name=country]');
 if(!type&&!country)return;
 const update=()=>form.querySelectorAll('[data-field-policy]').forEach(row=>{
  const mode=row.dataset[(type?.value||form.dataset.customerType)+'Mode'];row.hidden=mode==='hidden';
  const name=row.querySelector('[name]')?.name;
  let required=mode==='required';
  if(country&&name==='region')required&&=['IT','US','CA','AU'].includes(country.value);
  if(country&&name==='postal')required&&=['IT','DE','FR','ES','AT','CH','AU','US','CA','GB'].includes(country.value);
  row.querySelectorAll('input,select,textarea').forEach(input=>{input.disabled=row.hidden;input.required=required;});
  const mark=row.querySelector('[data-required-mark]');if(mark)mark.hidden=!required;
 });
 form.querySelector('[aria-invalid="true"]')?.focus();type?.addEventListener('change',update);country?.addEventListener('change',update);update();
})();
