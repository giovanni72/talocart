// Shared tab menu. Without JavaScript page tabs are links and in-page tabs are anchors to
// sections that all stay visible. Here every `.ui-tabs` track fades its overflowing edges,
// shows its edge buttons while it overflows and keeps the open tab in view; `[data-ui-tabs]`
// menus become ARIA tabs for the sections their "#id" links point to. Pages listen to `ui-tabs:change` for their own state: the
// menus are enhanced on DOMContentLoaded, after every deferred page script has run.
(() => {
  const selectedSelector = '[aria-current=page], [aria-current=location], [aria-selected=true]';

  const track = menu => {
    const list = menu.querySelector('.ui-tabs__list');
    if (!list) return () => {};
    const update = () => {
      // Measure against the whole rail: visible buttons must not sustain their
      // own overflow after the container grows. Settle their layout before reveal.
      const style = getComputedStyle(menu);
      const available = menu.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      menu.toggleAttribute('data-ui-tabs-scrollable', list.scrollWidth > available + 1);
      const edges = [];
      if (list.scrollLeft > 1) edges.push('start');
      if (list.scrollLeft + list.clientWidth < list.scrollWidth - 1) edges.push('end');
      if (edges.length) menu.dataset.uiTabsOverflow = edges.join(' ');
      else delete menu.dataset.uiTabsOverflow;
    };
    const reveal = () => {
      update();
      const open = list.querySelector(selectedSelector);
      if (open && list.scrollWidth > list.clientWidth) {
        list.scrollLeft = open.offsetLeft - list.offsetLeft - (list.clientWidth - open.offsetWidth) / 2;
      }
      update();
    };
    const smooth = !matchMedia('(prefers-reduced-motion: reduce)').matches;
    for (const [selector, direction] of [['.ui-tabs__scroll--start', -1], ['.ui-tabs__scroll--end', 1]]) {
      menu.querySelector(selector)?.addEventListener('click', () => {
        list.scrollBy({ left: direction * Math.max(list.clientWidth * .8, 120), behavior: smooth ? 'smooth' : 'auto' });
      });
    }
    list.addEventListener('scroll', update, { passive: true });
    if ('ResizeObserver' in window) {
      const observer = new ResizeObserver(reveal);
      observer.observe(menu);
      observer.observe(list);
    }
    reveal();
    return reveal;
  };

  const enhance = menu => {
    const list = menu.querySelector('.ui-tabs__list');
    const pairs = [...list.querySelectorAll('a.ui-tabs__tab[href^="#"]')]
      .map(tab => [tab, document.getElementById(decodeURIComponent(tab.hash.slice(1)))])
      .filter(([, panel]) => panel);
    if (!pairs.length) return track(menu);
    const tabs = pairs.map(([tab]) => tab);
    const current = tabs.find(tab => tab.hasAttribute('aria-current'));
    list.setAttribute('role', 'tablist');
    list.setAttribute('aria-label', menu.getAttribute('aria-label') || '');
    for (const [tab, panel] of pairs) {
      if (!tab.id) tab.id = `${panel.id}-tab`;
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-controls', panel.id);
      tab.removeAttribute('aria-current');
      panel.setAttribute('role', 'tabpanel');
      panel.setAttribute('aria-labelledby', tab.id);
      panel.dataset.uiTabsPanel = '';
      panel.tabIndex = 0;
    }
    const reveal = track(menu);
    let open;
    const activate = (id, { focus = false, history = false } = {}) => {
      const pair = pairs.find(([, panel]) => panel.id === id) || pairs[0];
      for (const [tab, panel] of pairs) {
        const selected = tab === pair[0];
        tab.setAttribute('aria-selected', String(selected));
        tab.tabIndex = selected ? 0 : -1;
        panel.hidden = !selected;
      }
      if (focus) pair[0].focus();
      if (history) window.history.replaceState(window.history.state, '', `#${pair[1].id}`);
      reveal();
      if (open !== pair[1].id) {
        open = pair[1].id;
        menu.dispatchEvent(new CustomEvent('ui-tabs:change', { bubbles: true, detail: { panel: pair[1], tab: pair[0] } }));
      }
    };
    tabs.forEach((tab, index) => {
      tab.addEventListener('click', event => {
        event.preventDefault();
        activate(tab.hash.slice(1), { history: true });
      });
      tab.addEventListener('keydown', event => {
        const next = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: tabs.length - 1 }[event.key];
        if (next !== undefined) {
          event.preventDefault();
          activate(tabs[(next + tabs.length) % tabs.length].hash.slice(1), { focus: true, history: true });
        } else if (event.key === ' ') {
          event.preventDefault();
          activate(tab.hash.slice(1), { history: true });
        }
      });
    });
    const owner = element => pairs.find(([, panel]) => panel.contains(element))?.[1].id || '';
    // Native validation fires for every invalid field: open the first one's panel before the
    // browser focuses it, ignoring the rest of the same validation pass.
    let validating = false;
    document.addEventListener('invalid', event => {
      const id = owner(event.target);
      if (!id || validating) return;
      validating = true;
      setTimeout(() => { validating = false; }, 0);
      if (id !== open) activate(id);
    }, true);
    // The hash may name a panel or an anchor inside one.
    const fromHash = () => {
      const target = location.hash.length > 1 && document.getElementById(decodeURIComponent(location.hash.slice(1)));
      return target ? owner(target) : '';
    };
    const invalid = pairs.find(([, panel]) => panel.querySelector('[aria-invalid=true]'))?.[1].id || '';
    const initial = menu.dataset.uiTabsInitial && document.getElementById(menu.dataset.uiTabsInitial) ? menu.dataset.uiTabsInitial : '';
    activate(invalid || initial || fromHash() || current?.hash.slice(1) || pairs[0][1].id);
    window.addEventListener('hashchange', () => { const id = fromHash(); if (id) activate(id); });
    menu.dataset.uiTabsReady = '';
    return reveal;
  };

  const start = () => {
    for (const menu of document.querySelectorAll('.ui-tabs')) {
      if (menu.matches('[data-ui-tabs]')) enhance(menu);
      else track(menu);
    }
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
