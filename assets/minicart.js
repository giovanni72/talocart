// Native dialog owns focus/inertness; HTMX owns transport. The link remains a no-JS fallback.
(() => {
  const dialog = document.querySelector('#minicart');
  const link = document.querySelector('[data-cart-link]');
  if (!dialog || !link || dialog.dataset.ready || typeof dialog.showModal !== 'function' || !window.htmx) return;
  dialog.dataset.ready = 'true';
  link.setAttribute('aria-haspopup', 'dialog');
  link.setAttribute('aria-controls', dialog.id);
  link.setAttribute('aria-expanded', 'false');
  const content = dialog.querySelector('#minicart-content');
  const loading = dialog.querySelector('#minicart-loading');
  const error = dialog.querySelector('#minicart-error');
  const close = dialog.querySelector('[data-minicart-close]');
  // A removal may finish after close. Keep its form alive even if the dialog reopens.
  let removing = null, changed = false;
  const refreshPage = () => {
    if (!changed || dialog.open || removing !== null) return;
    changed = false;
    // Checkout details owns its refresh so unsaved fields stay in the same document.
    if (!document.getElementById('checkout-details') && document.querySelector('[data-cart-item], #checkout-summary')) location.reload();
  };
  const pending = value => {
    loading.hidden = !value;
    content.setAttribute('aria-busy', String(value));
  };
  link.addEventListener('click', event => {
    if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    if (dialog.open) return;
    if (removing !== null) {
      content.setAttribute('aria-busy', 'true');
      dialog.showModal();
      link.setAttribute('aria-expanded', 'true');
      return;
    }
    content.replaceChildren();
    error.hidden = true;
    pending(true);
    dialog.showModal();
    link.setAttribute('aria-expanded', 'true');
    htmx.trigger(link, 'minicart-open');
  });
  dialog.querySelectorAll('[data-minicart-close]').forEach(button => button.addEventListener('click', () => dialog.close()));
  dialog.addEventListener('click', event => {
    const rect = dialog.getBoundingClientRect();
    if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) dialog.close();
  });
  dialog.addEventListener('keydown', event => {
    if (event.key !== 'Tab') return;
    const controls = Array.from(dialog.querySelectorAll('a[href],button:not([disabled]),[tabindex="0"]')).filter(element => element.getClientRects().length);
    const first = controls[0], last = controls[controls.length - 1];
    if (!first) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault(); last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault(); first.focus();
    }
  });
  dialog.addEventListener('close', () => {
    htmx.trigger(link, 'htmx:abort');
    pending(false);
    link.setAttribute('aria-expanded', 'false');
    link.focus({ preventScroll: true });
    refreshPage();
  });
  document.addEventListener('htmx:beforeRequest', event => {
    const form = event.detail.elt;
    if (!(form instanceof HTMLFormElement) || !content.contains(form)) return;
    removing = Array.from(content.querySelectorAll('form.minicart-remove')).indexOf(form);
    content.setAttribute('aria-busy', 'true');
    document.dispatchEvent(new Event('minicart:changing'));
  });
  document.addEventListener('htmx:afterSwap', event => {
    if (event.detail.target !== content) return;
    pending(false);
    const result = content.querySelector('[data-minicart-count]');
    if (!result) { content.replaceChildren(); error.hidden = false; return; }
    document.querySelectorAll('[data-cart-count]').forEach(count => { count.textContent = result.dataset.minicartCount; });
    if (removing === null) return;
    changed = true;
    const removes = content.querySelectorAll('[data-minicart-remove]');
    if (dialog.open) (removes[Math.min(removing, removes.length - 1)] || close).focus();
    removing = null;
    document.dispatchEvent(new Event('minicart:changed'));
    refreshPage();
  });
  const failed = event => {
    const action = removing !== null && content.contains(event.detail.elt);
    if (event.detail.elt !== link && !action) return;
    removing = null;
    pending(false);
    content.replaceChildren();
    error.hidden = false;
    if (action) {
      if (dialog.open) close.focus();
      // A lost response does not prove that the server rolled the mutation back.
      changed = true;
      document.dispatchEvent(new Event('minicart:changed'));
      refreshPage();
    }
  };
  ['htmx:responseError', 'htmx:sendError', 'htmx:timeout'].forEach(name => document.addEventListener(name, failed));
})();
