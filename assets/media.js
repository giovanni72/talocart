(() => {
  'use strict';
  const pending = new Set(document.querySelectorAll('img[data-media-pending]'));
  let attempts = 0;
  if (pending.size) {
    const timer = setInterval(async () => {
      if (!pending.size || ++attempts > 60) { clearInterval(timer); return; }
      for (const img of pending) {
        try {
          const response = await fetch(img.getAttribute('src'), { method: 'HEAD', cache: 'no-store' });
          if (response.ok && response.headers.get('x-media-ready') === 'true') {
            img.src = img.getAttribute('src').split('?')[0] + '?ready=' + Date.now();
            pending.delete(img);
          }
        } catch { /* The explicit refresh link remains available. */ }
      }
    }, 2000);
  }
  const checks = [...document.querySelectorAll('[data-media-select]')];
  const form = document.querySelector('#media-selection');
  if (!form || !checks.length) return;
  const toolbar = form.querySelector('.media-multiple');
  const ids = form.querySelector('#media-selected-ids');
  const count = form.querySelector('#media-selection-count');
  const countLabels = [...document.querySelector('#media-selection-labels').content.querySelectorAll('span')].map(label => label.textContent);
  checks.forEach(check => {
    check.closest('label').hidden = false;
    check.addEventListener('change', () => {
      const selected = checks.filter(item => item.checked);
      ids.value = selected.map(item => item.dataset.mediaSelect).join(',');
      count.textContent = countLabels[selected.length];
      toolbar.hidden = selected.length === 0;
      checks.forEach(item => item.disabled = !item.checked && selected.length >= 12);
    });
  });
  form.addEventListener('submit', event => { if (!ids.value) event.preventDefault(); });
})();
