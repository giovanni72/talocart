(() => {
  'use strict';
  const states = ['due', 'checking', 'paid', 'review', 'refunded', 'cancelled'];
  const terminal = state => !['due', 'checking'].includes(state);
  function pending(reference, value) {
    try {
      const key = `talocart-payment-pending:${reference}`;
      if (value === undefined) return sessionStorage.getItem(key) === '1';
      if (value) sessionStorage.setItem(key, '1'); else sessionStorage.removeItem(key);
    } catch { /* Optional hint; the server remains authoritative. */ }
    return false;
  }
  // One bounded, serial JSON poller for both checkout and the legacy receipt.
  // A callback may fetch HTML once on a transition; it shares the retry budget.
  function watch({url, onStatus, onStop, onBusy = () => {}}) {
    let stopped = true, inFlight = false, timer, deadline, failures = 0, generation = 0, controller;
    function stop(reason) {
      stopped = true; generation++; clearTimeout(timer); controller?.abort();
      onStop(reason);
    }
    function schedule() { timer = setTimeout(() => poll(false), 2000); }
    async function poll(manual) {
      if (inFlight || stopped) return;
      clearTimeout(timer);
      if (Date.now() >= deadline) { stop('long'); return; }
      if (document.hidden && !manual) { schedule(); return; }
      inFlight = true; onBusy(true);
      const current = generation;
      controller = new AbortController();
      try {
        const response = await fetch(url, {credentials: 'same-origin', cache: 'no-store', redirect: 'error', signal: AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]), headers: {Accept: 'application/json'}});
        if (!response.ok) throw new Error('Status unavailable');
        const result = await response.json();
        if (stopped || current !== generation) return;
        if (!states.includes(result.state) || typeof result.can_resume !== 'boolean' || (result.state === 'paid' && (typeof result.receipt_url !== 'string' || !result.receipt_url))) throw new Error('Invalid payment status');
        await onStatus(result, controller.signal);
        failures = 0;
      } catch {
        if (!stopped && current === generation && ++failures >= 3) stop('network');
      } finally {
        inFlight = false; onBusy(false);
        if (!stopped && current === generation) schedule();
      }
    }
    function start(immediate = true) {
      if (inFlight) return;
      clearTimeout(timer); stopped = false; failures = 0; deadline = Date.now() + 60000;
      if (immediate) poll(true); else schedule();
    }
    window.addEventListener('pagehide', () => stop(), {once: true});
    return {start, stop};
  }
  window.TalocartPaymentStatus = {watch, pending, terminal};
})();
