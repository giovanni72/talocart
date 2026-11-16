// Product sections are shared ui-tabs panels (assets/ui-tabs.js). Without JavaScript every
// section stays visible; here the open section is kept for the server after a submit.
document.addEventListener('ui-tabs:change', event => {
  const editor = event.target.closest('[data-product-tabs]');
  const field = editor?.querySelector('[name="editor_tab"]');
  const key = event.detail.panel.dataset.productPanel;
  if (field && key) field.value = key;
});

// Native submissions preserve the saved quantity while stock control is disabled.
document.addEventListener('change', event => {
  if (!event.target.matches('#product-manage-stock')) return;
  const form = event.target.form;
  const managed = event.target.checked;
  const quantity = form.querySelector('[data-managed-stock]');
  const preserved = form.querySelector('[data-preserved-stock]');
  if (quantity) quantity.disabled = !managed;
  if (preserved) preserved.disabled = managed;
  const stockHelp = form.querySelector('[data-stock-managed-help]');
  if (stockHelp) stockHelp.hidden = !managed;
  form.querySelector('[data-stock-unmanaged]').hidden = managed;
});
