// Native links work without JavaScript. HTMX replaces the product detail only;
// synchronize the header links with the pushed variant/photo URL.
(() => {
  const update = () => {
    const current = new URL(window.location.href);
    const navigation = document.querySelector('.store-languages');
    if (!navigation) return;
    const base = navigation.dataset.currentPrefix;
    const path = base && (current.pathname === base || current.pathname === base + '/') ? '/' : base && current.pathname.startsWith(base + '/') ? current.pathname.slice(base.length) : current.pathname;
    for (const link of document.querySelectorAll('.store-languages a[hreflang]')) {
      const prefix = link.dataset.prefix;
      link.href = (path === '/' && prefix ? prefix : prefix + path) + current.search;
    }
  };
  document.addEventListener('htmx:afterSettle', update);
  document.addEventListener('htmx:pushedIntoHistory', update);
  document.addEventListener('talocart:photo', update);
  window.addEventListener('popstate', update);
})();
