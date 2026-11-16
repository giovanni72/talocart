// Progressive enhancement: source groups remain readable inline without scripts.
(() => {
  const groups = new Map();
  const active = new Map();
  const viewport = scope => {
    let host = [...scope.children].find(node => node.hasAttribute('data-ui-toast-viewport'));
    if (!host) {
      host = document.createElement('div');
      host.className = 'ui-toast-viewport';
      host.setAttribute('data-ui-toast-viewport', '');
      if (scope.matches('dialog')) scope.prepend(host);
      else scope.append(host);
    }
    return host;
  };
  const initialize = () => {
    // A portal belongs to its original fragment, including after an HTMX replacement.
    for (const [group, anchor] of groups) {
      if (!anchor.isConnected || !group.isConnected) {
        group.remove(); anchor.remove(); groups.delete(group);
      }
    }
    for (const [toast, state] of active) {
      if (!toast.isConnected || toast.hidden) { state.dispose(); active.delete(toast); }
    }
    document.querySelectorAll('[data-ui-toasts]').forEach(group => {
      if (!groups.has(group)) {
        const anchor = document.createComment('toast source');
        const scope = group.closest('dialog') || document.body;
        group.before(anchor); groups.set(group, anchor);
        group.classList.add('ui-toasts--active');
        viewport(scope).append(group);
      }
      group.querySelectorAll('[data-ui-toast]').forEach(toast => {
        if (toast.hidden || active.has(toast)) return;
        const close = toast.querySelector('[data-ui-toast-close]');
        if (!close) return;
        const host = group.parentElement;
        const scope = host.closest('dialog') || document;
        const controller = new AbortController();
        const options = {signal: controller.signal};
        const value = Number(toast.dataset.duration);
        const automatic = Number.isFinite(value) && value > 0;
        let remaining = automatic ? Math.max(1000, value) : 0;
        let timer, started;
        const pause = () => {
          if (timer) { clearTimeout(timer); timer = null; remaining = Math.max(0, remaining - (performance.now() - started)); }
        };
        const dispose = () => { pause(); controller.abort(); };
        const dismiss = () => {
          const focused = toast.contains(document.activeElement);
          const next = [...host.querySelectorAll('[data-ui-toast]:not([hidden]) [data-ui-toast-close]')].find(button => button !== close);
          dispose(); active.delete(toast);
          const described = toast.id && [...document.querySelectorAll('[aria-describedby]')].some(element => element.getAttribute('aria-describedby').split(/\s+/).includes(toast.id));
          if (toast.hasAttribute('data-ui-toast-reusable') || described) toast.hidden = true;
          else toast.remove();
          if (focused) {
            const target = next || scope.querySelector('main, [data-minicart-close]');
            if (target) {
              if (!next && target.matches('main') && !target.hasAttribute('tabindex')) {
                target.setAttribute('tabindex', '-1');
                target.addEventListener('blur', () => target.removeAttribute('tabindex'), {once:true});
              }
              target.focus({preventScroll:true});
            }
          }
        };
        const resume = () => {
          pause();
          if (!automatic || document.hidden || toast.matches(':hover') || toast.contains(document.activeElement)) return;
          started = performance.now(); timer = setTimeout(dismiss, remaining);
        };
        active.set(toast, {dispose});
        close.hidden = false;
        close.addEventListener('click', dismiss, options);
        toast.addEventListener('mouseenter', pause, options);
        toast.addEventListener('mouseleave', resume, options);
        toast.addEventListener('focusin', pause, options);
        toast.addEventListener('focusout', () => queueMicrotask(() => { if (toast.isConnected) resume(); }), options);
        document.addEventListener('visibilitychange', resume, options);
        resume();
      });
    });
    document.querySelectorAll('[data-ui-toast-viewport]').forEach(host => {
      if (!host.childElementCount) host.remove();
    });
  };
  initialize();
  new MutationObserver(initialize).observe(document.body, {childList:true, subtree:true, attributes:true, attributeFilter:['hidden']});
})();
