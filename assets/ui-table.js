// Native forms and image links remain the fallback when JavaScript is unavailable.
(() => {
  for (const picker of document.querySelectorAll('[data-product-columns]')) {
    const form = picker.querySelector('[data-product-columns-form]');
    const table = document.querySelector('[data-product-selection]');
    if (!form || !table) continue;
    const controls = [...form.querySelectorAll('input[name="columns"]')];
    const apply = form.querySelector('[data-columns-apply]');
    const reset = form.querySelector('[data-columns-reset]');
    const error = form.querySelector('[data-columns-error]');
    let saved = controls.map(control => control.checked);
    let busy = false;
    const render = () => {
      const visible = new Set(["actions", ...controls.filter(control => control.checked).map(control => control.value)]);
      for (const cell of table.querySelectorAll('[data-product-column]')) {
        cell.hidden = !visible.has(cell.dataset.productColumn);
      }
      const empty = table.querySelector('[data-product-empty]');
      if (empty) empty.colSpan = visible.size;
    };
    const save = async () => {
      if (busy) return;
      const body = new URLSearchParams(new FormData(form));
      const focused = document.activeElement;
      busy = true;
      error.hidden = true;
      form.setAttribute('aria-busy', 'true');
      controls.forEach(control => { control.disabled = true; });
      reset.disabled = true;
      render();
      try {
        const response = await fetch(form.action, {
          method: 'POST', body, headers: {Accept: 'application/json'},
          credentials: 'same-origin', redirect: 'error', signal: AbortSignal.timeout(10000),
        });
        if (!response.ok) throw new Error('Preference save failed');
        saved = controls.map(control => control.checked);
      } catch {
        controls.forEach((control, index) => { control.checked = saved[index]; });
        render();
        error.hidden = false;
      } finally {
        controls.forEach(control => { control.disabled = false; });
        reset.disabled = false;
        form.removeAttribute('aria-busy');
        if (document.activeElement === document.body && form.contains(focused)) focused.focus({preventScroll: true});
        busy = false;
      }
    };
    form.addEventListener('change', save);
    form.addEventListener('submit', event => { event.preventDefault(); save(); });
    reset.addEventListener('click', () => {
      if (busy) return;
      controls.forEach(control => { control.checked = true; });
      save();
    });
    apply.hidden = true;
    reset.hidden = false;
    document.addEventListener('click', event => {
      if (!picker.contains(event.target)) picker.open = false;
    });
    picker.addEventListener('keydown', event => {
      if (event.key === 'Escape') {
        picker.open = false;
        picker.querySelector('summary').focus();
      }
    });
  }

  let openedInfo = null;
  let hideImage = () => {};
  for (const details of document.querySelectorAll('[data-ui-info]')) {
    const summary = details.querySelector('summary');
    const panel = details.querySelector('.ui-info__content');
    if (!summary || !panel) continue;
    details.dataset.uiInfoEnhanced = '';
    panel.classList.add('ui-info__content--floating');
    panel.hidden = true;
    panel.tabIndex = 0;
    document.body.append(panel);
    const close = () => {
      details.open = false;
      panel.hidden = true;
      summary.removeAttribute('aria-describedby');
      if (openedInfo?.details === details) openedInfo = null;
    };
    const position = () => {
      if (!summary.getClientRects().length) { close(); return; }
      const anchor = summary.getBoundingClientRect();
      const region = details.closest('.ui-table-region');
      const clip = region ? region.getBoundingClientRect() : {left: 0, right: innerWidth};
      if (anchor.bottom <= 0 || anchor.top >= innerHeight || anchor.right <= Math.max(0,clip.left) || anchor.left >= Math.min(innerWidth,clip.right)) { close(); return; }
      const box = panel.getBoundingClientRect();
      panel.style.left = Math.max(12, Math.min(anchor.right-box.width, innerWidth-box.width-12)) + 'px';
      const top = anchor.bottom+8+box.height <= innerHeight-12 ? anchor.bottom+8 : anchor.top-box.height-8;
      panel.style.top = Math.max(12,Math.min(top,innerHeight-box.height-12)) + 'px';
    };
    details.addEventListener('toggle', () => {
      if (!details.open) { close(); return; }
      hideImage();
      if (openedInfo && openedInfo.details !== details) openedInfo.close();
      panel.hidden = false;
      openedInfo = {details, panel, summary, close, position};
      summary.setAttribute('aria-describedby', panel.id);
      position();
    });
    summary.addEventListener('keydown', event => {
      if (event.key === 'ArrowDown' && details.open) { event.preventDefault(); panel.focus(); }
    });
  }
  document.addEventListener('pointerdown', event => {
    if (openedInfo && !openedInfo.details.contains(event.target) && !openedInfo.panel.contains(event.target)) openedInfo.close();
  });
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && openedInfo) {
      const summary = openedInfo.summary;
      openedInfo.close(); summary.focus({preventScroll: true});
    }
  });
  document.addEventListener('scroll', () => openedInfo?.position(), true);
  window.addEventListener('resize', () => openedInfo?.position());
  document.addEventListener('change', () => openedInfo?.position());

  const links = document.querySelectorAll('[data-ui-image-preview]');
  if (!links.length) return;
  const tooltip = document.createElement('div');
  tooltip.id = 'ui-image-tooltip';
  tooltip.className = 'ui-image-tooltip';
  tooltip.setAttribute('role', 'tooltip');
  tooltip.hidden = true;
  const image = document.createElement('img');
  tooltip.append(image);
  document.body.append(tooltip);
  let active = null;
  let timer;
  const hide = () => {
    clearTimeout(timer);
    if (active) active.removeAttribute('aria-describedby');
    active = null;
    tooltip.hidden = true;
  };
  hideImage = hide;
  const position = () => {
    if (!active) return;
    const anchor = active.getBoundingClientRect();
    const box = tooltip.getBoundingClientRect();
    const right = anchor.right + 8;
    const left = right + box.width <= innerWidth - 8 ? right : anchor.left - box.width - 8;
    tooltip.style.left = Math.max(8, Math.min(left, innerWidth - box.width - 8)) + 'px';
    tooltip.style.top = Math.max(8, Math.min(anchor.top + (anchor.height - box.height) / 2, innerHeight - box.height - 8)) + 'px';
  };
  const show = link => {
    openedInfo?.close();
    clearTimeout(timer);
    if (active && active !== link) hide();
    active = link;
    const thumbnail = link.querySelector('img');
    image.alt = thumbnail.dataset.previewName;
    image.src = link.href;
    tooltip.hidden = false;
    link.setAttribute('aria-describedby', tooltip.id);
    position();
  };
  const leave = () => {
    if (active === document.activeElement) return;
    timer = setTimeout(hide, 150);
  };
  image.addEventListener('load', position);
  image.addEventListener('error', hide);
  tooltip.addEventListener('pointerenter', () => clearTimeout(timer));
  tooltip.addEventListener('pointerleave', leave);
  for (const link of links) {
    link.addEventListener('pointerenter', event => { if (event.pointerType === 'mouse') show(link); });
    link.addEventListener('pointerleave', leave);
    link.addEventListener('focus', () => show(link));
    link.addEventListener('blur', hide);
    link.addEventListener('click', event => { event.preventDefault(); show(link); });
  }
  document.addEventListener('keydown', event => { if (event.key === 'Escape') hide(); });
  document.addEventListener('pointerdown', event => {
    if (active && !active.contains(event.target) && !tooltip.contains(event.target)) hide();
  });
  // Keep the preview attached while scrolling, and dismiss it when its trigger is clipped.
  document.addEventListener('scroll', () => {
    if (!active) return;
    const anchor = active.getBoundingClientRect();
    const region = active.closest('.ui-table-region');
    const clip = region ? region.getBoundingClientRect() : {left: 0, right: innerWidth};
    if (anchor.right <= Math.max(0, clip.left) || anchor.left >= Math.min(innerWidth, clip.right)
        || anchor.bottom <= 0 || anchor.top >= innerHeight) hide();
    else position();
  }, true);
  window.addEventListener('resize', hide);
})();
