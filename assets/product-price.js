// Delegated handlers also cover variant forms replaced by HTMX.
(() => {
  const requests = new WeakMap();
  const inputFor = event => {
    const input = event.detail?.elt || event.target;
    return input?.matches?.('#cart-quantity[hx-post]') ? input : null;
  };
  const display = (input, ready, failed) => {
    const form = input.form;
    form.querySelector('button[type=submit]:not([formaction])').disabled = !ready;
    form.querySelector('#product-price-error').hidden = !failed;
    form.querySelector('[data-price-retry]').hidden = !failed;
    for (const id of ['product-unit-price', 'product-unit-net', 'product-unit-tax']) {
      const price = document.getElementById(id);
      if (price) price.hidden = !ready;
    }
  };
  document.addEventListener('input', event => {
    const input = inputFor(event);
    if (input) display(input, false, false);
  });
  document.addEventListener('htmx:beforeRequest', event => {
    const input = inputFor(event);
    if (!input) return;
    requests.set(event.detail.xhr, { input, quantity: input.value });
    display(input, false, false);
  });
  document.addEventListener('htmx:beforeSwap', event => {
    const request = requests.get(event.detail.xhr);
    if (request && request.input.value !== request.quantity) event.detail.shouldSwap = false;
  });
  document.addEventListener('htmx:afterRequest', event => {
    const request = requests.get(event.detail.xhr);
    if (!request || !request.input.isConnected || request.input.value !== request.quantity) return;
    // A successful HTTP response must contain the expected price fragment.
    const ready = event.detail.successful && event.detail.xhr.responseText.includes('id="product-unit-price"');
    display(request.input, ready, !ready);
  });
  document.addEventListener('click', event => {
    const button = event.target.closest('[data-price-retry]');
    if (!button) return;
    const input = button.form.querySelector('#cart-quantity');
    if (!input.reportValidity()) return;
    input.focus({ preventScroll: true });
    htmx.trigger(input, 'price-retry');
  });
})();
