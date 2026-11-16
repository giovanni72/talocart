// Photo links remain native navigation without JavaScript. Only the chosen image
// is downloaded; prices, quantity, variant forms and their pending requests stay intact.
(() => {
  const show = (gallery, link) => {
    const image = gallery.querySelector('.store-detail-photo img');
    if (!image || !link.dataset.image) return false;
    image.srcset = link.dataset.srcset || '';
    image.src = link.dataset.image;
    for (const photo of gallery.querySelectorAll('[data-product-photo]')) {
      if (photo === link) photo.setAttribute('aria-current', 'true');
      else photo.removeAttribute('aria-current');
    }
    document.dispatchEvent(new CustomEvent('talocart:photo'));
    return true;
  };
  document.addEventListener('click', event => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest('a[data-product-photo]');
    const gallery = link?.closest('.store-product-gallery');
    if (!gallery || link.target || link.hasAttribute('download') || !gallery.querySelector('.store-detail-photo img')) return;
    const url = new URL(link.href);
    if (url.origin !== location.origin || url.pathname !== location.pathname) return;
    const quantity = document.getElementById('cart-quantity');
    if (quantity?.validity.valid) url.searchParams.set('quantity', quantity.value);
    event.preventDefault();
    if (link.hasAttribute('aria-current')) return;
    const token = gallery.dataset.galleryHistory ||= crypto.randomUUID();
    // Tag the original entry as well, so Back restores its image without asking
    // HTMX to restore/replace the product and its live form state.
    history.replaceState({ ...history.state, talocartGallery: token }, '', location.href);
    history.pushState({ talocartGallery: token }, '', url);
    show(gallery, link);
  });
  window.addEventListener('popstate', event => {
    const gallery = document.querySelector('.store-product-gallery');
    if (!event.state?.talocartGallery) return;
    if (event.state.talocartGallery !== gallery?.dataset.galleryHistory) {
      // A variant HTMX swap (or another page) replaced the gallery. Re-enter its
      // server-validated state instead of showing the new variant under an old URL.
      event.stopImmediatePropagation();
      location.reload();
      return;
    }
    const key = new URL(location.href).searchParams.get('photo') || '';
    const links = [...gallery.querySelectorAll('[data-product-photo]')];
    const link = links.find(photo => photo.dataset.productPhoto === key) || (!key ? links[0] : null);
    if (!link) return;
    // Capture before HTMX's history handler; other navigations retain its normal behavior.
    event.stopImmediatePropagation();
    show(gallery, link);
  }, true);
})();
