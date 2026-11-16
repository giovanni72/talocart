(() => {
  const status = document.getElementById('checkout-update-status');
  if (!status || !window.fetch || !window.AbortController) return;
  const message = status.querySelector('span');
  const reload = status.querySelector('a');
  let pending = null;
  let running = false;
  let failed = false;
  let revision;
  const form = () => document.getElementById('checkout-methods');
  const setup = () => {
    const current = form();
    if (!current) return;
    current.querySelector('[data-co-update]').hidden = true;
    revision = current.elements.revision.value;
  };
  const lockConfirmation = () => {
    const confirm = document.getElementById('checkout-confirm');
    if (confirm) confirm.disabled = true;
  };
  const choices = current => ({
    shipping: new FormData(current).get('shipping_method_id') || '',
    payment: new FormData(current).get('payment_method_id') || ''
  });
  const update = async () => {
    if (running || failed) return;
    running = true;
    lockConfirmation();
    message.textContent = status.dataset.loading;
    document.getElementById('checkout-methods-region').setAttribute('aria-busy', 'true');
    try {
      while (pending) {
        const selected = pending;
        pending = null;
        const current = form();
        const body = new URLSearchParams({
          csrf: current.elements.csrf.value,
          revision,
          action: 'update',
          shipping_method_id: selected.shipping,
          payment_method_id: selected.payment
        });
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), 15000);
        let response;
        let html;
        try {
          response = await fetch(current.getAttribute('action'), {
            method: 'POST', credentials: 'same-origin',
            headers: {'HX-Request': 'true', 'Content-Type': 'application/x-www-form-urlencoded'},
            body, signal: controller.signal
          });
          html = await response.text();
        } finally {
          clearTimeout(timeout);
        }
        const parsed = new DOMParser().parseFromString(html, 'text/html');
        const region = parsed.getElementById('checkout-methods-region');
        if (!region || response.redirected || ![200, 409, 422].includes(response.status)) throw new Error('Checkout update unavailable');
        revision = region.querySelector('[name="revision"]').value;
        // Serialize changes: an older response can never replace a newer choice.
        // Keep the current controls until the last queued change has completed.
        if (pending && response.ok) continue;
        pending = null;
        const focused = document.activeElement;
        const focusId = focused && focused.closest('#checkout-methods-region') ? focused.id : '';
        document.getElementById('checkout-methods-region').replaceWith(region);
        setup();
        if (focusId) (document.getElementById(focusId) || form().querySelector('input[type=radio]:checked'))?.focus({preventScroll: true});
        message.textContent = response.ok ? status.dataset.done : '';
        if (!response.ok) {
          failed = true;
          lockConfirmation();
          reload.hidden = false;
        }
      }
    } catch {
      pending = null;
      failed = true;
      message.textContent = status.dataset.error;
      reload.hidden = false;
      lockConfirmation();
    } finally {
      running = false;
      document.getElementById('checkout-methods-region')?.removeAttribute('aria-busy');
    }
  };
  document.addEventListener('change', event => {
    if (!event.target.matches('#checkout-methods input[type="radio"]')) return;
    pending = choices(form());
    lockConfirmation();
    void update();
  });
  document.addEventListener('submit', event => {
    if (event.target.id === 'checkout-methods' && (running || failed)) event.preventDefault();
  });
  setup();
})();
