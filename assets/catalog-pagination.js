// The select belongs to the shared GET form, which contains no page parameter.
(() => {
  const select = document.getElementById('catalog-page-size');
  const form = select?.form;
  if (!form || typeof form.requestSubmit !== 'function') return;
  select.addEventListener('change', () => form.requestSubmit());
  const apply = document.querySelector('[data-catalog-page-apply]');
  if (apply) apply.hidden = true;
})();
