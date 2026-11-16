// Native GET form remains usable without JavaScript; changing size starts at page one.
(() => {
  for (const form of document.querySelectorAll('[data-product-page-size]')) {
    const select = form.querySelector('select[name="per_page"]');
    if (!select || typeof form.requestSubmit !== 'function') continue;
    select.addEventListener('change', () => form.requestSubmit());
    const button = form.querySelector('button[type="submit"]');
    if (button) button.hidden = true;
  }
})();
