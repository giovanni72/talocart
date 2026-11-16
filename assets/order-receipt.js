(() => {
  'use strict';
  let receipt = document.getElementById('order-receipt');
  if (!receipt) return;
  const paymentStatus = window.TalocartPaymentStatus;
  const terminal = paymentStatus.terminal;
  const announcement = document.getElementById('receipt-announcement');
  // Older immutable sessions may lack the return hint. This local hint never asserts payment.
  let submitted = paymentStatus.pending(receipt.dataset.reference) || new URL(location.href).searchParams.get('payment_return') === '1';
  if (submitted && ['due', 'checking'].includes(receipt.dataset.paymentState) && receipt.dataset.paymentUrl) { location.replace(receipt.dataset.paymentUrl); return; }
  if (submitted && receipt.dataset.paymentState === 'due' && !['expired', 'failed'].includes(receipt.dataset.attemptState)) {
    receipt.dataset.paymentState = 'checking';
    receipt.dataset.waiting = receipt.dataset.verifyingWaiting;
    const url = new URL(receipt.dataset.statusUrl, location.origin);
    url.searchParams.set('payment_return', '1');
    receipt.dataset.statusUrl = url.pathname + url.search;
    document.getElementById('online-payment-verifying').hidden = false;
    document.getElementById('online-payment-due')?.remove();
    receipt.querySelector('[data-payment-instructions]')?.remove();
    document.getElementById('receipt-title').textContent = receipt.dataset.checkingTitle;
    document.getElementById('order-payment-state').textContent = receipt.dataset.checkingTitle;
    document.getElementById('online-payment-check-status').href = receipt.dataset.statusUrl;
    history.replaceState(null, '', receipt.dataset.statusUrl);
  }
  function clearHint() { paymentStatus.pending(receipt.dataset.reference, false); }
  function stop(reason) {
    const warning = document.getElementById('online-payment-refresh-warning');
    if (warning && reason) { warning.textContent = receipt.dataset[reason === 'long' ? 'waiting' : 'network']; warning.hidden = false; }
    if (reason) {
      document.getElementById('online-payment-refresh-help').hidden = true;
      receipt.querySelectorAll('.ui-spinner').forEach(spinner => { spinner.hidden = true; });
    }
  }
  if (terminal(receipt.dataset.paymentState)) { clearHint(); return; }
  document.getElementById('online-payment-refresh-help').hidden = false;
  const poller = paymentStatus.watch({
    url: receipt.dataset.paymentStatusUrl,
    onStop: stop,
    onStatus: async (result, signal) => {
      const state = result.state === 'due' && submitted && !result.can_resume ? 'checking' : result.state;
      if (result.can_resume) { submitted = false; clearHint(); }
      if (state === receipt.dataset.paymentState) return;
      const response = await fetch(receipt.dataset.statusUrl, {credentials: 'same-origin', cache: 'no-store', redirect: 'error', signal: AbortSignal.any([signal, AbortSignal.timeout(5000)]), headers: {Accept: 'text/html'}});
      if (!response.ok) throw new Error('Status unavailable');
      const next = new DOMParser().parseFromString(await response.text(), 'text/html').getElementById('order-receipt');
      if (!next || next.dataset.reference !== receipt.dataset.reference) throw new Error('Invalid receipt');
      if (signal.aborted) return;
      if (next.dataset.paymentState === 'due' && ['expired', 'failed'].includes(next.dataset.attemptState)) clearHint();
      if (next.dataset.paymentState !== receipt.dataset.paymentState) {
        const focusWasInside = receipt.contains(document.activeElement);
        receipt.replaceWith(next); receipt = next;
        const title = document.getElementById('receipt-title');
        document.title = `${title.textContent} ${receipt.dataset.reference} · ${document.querySelector('.site-footer').dataset.storeName}`;
        announcement.textContent = receipt.dataset.paymentState === 'paid' ? receipt.dataset.confirmed : title.textContent;
        if (focusWasInside) title.focus({preventScroll: true});
      }
      if (terminal(receipt.dataset.paymentState)) { clearHint(); poller.stop(); return; }
      document.getElementById('online-payment-refresh-help').hidden = false;
    }
  });
  poller.start(false);
})();
