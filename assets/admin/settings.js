(() => {
  'use strict';
  const root = document.querySelector('[data-settings-editor]');
  const form = document.querySelector('#settings-editor');
  if (!root || !form) return;
  const panels = [...root.querySelectorAll('[data-settings-panel]')];
  const tabField = form.elements.namedItem('tab');
  const baseline = new URLSearchParams(form.dataset.baseline);
  const helpers = new Set(['module_install_password', 'csrf', 'settings_revision', 'regional_revision', 'fiscal_revision', 'tab', 'action', 'media_query', 'media_page', 'media_target', 'media_editor', 'media_folder', 'media_folders_page', 'media_upload_failed', 'media_format', 'media_usage', 'media_sort', 'media_selected', 'media_saved', 'picker_image']);
  const fiscal = new Set(['currency', 'tax_country', 'mode', 'default_class_id']);
  const writable = key => fiscal.has(key) ? form.dataset.taxWrite === 'true' : form.dataset.settingsWrite === 'true';
  const clean = pairs => [...pairs].filter(([key]) => !helpers.has(key) && writable(key)).map(([key, value]) => JSON.stringify([key, value])).sort().join('\n');
  const original = clean(baseline);
  const status = document.querySelector('#settings-change-status');
  let dirty = false;
  let submitting = false;
  // Panels are shared ui-tabs sections (assets/ui-tabs.js); the server keeps the open one.
  const tabFor = panel => document.getElementById(`settings-tab-${panel.dataset.settingsPanel}`);
  root.addEventListener('ui-tabs:change', event => {
    const id = event.detail.panel.dataset.settingsPanel;
    if (id) tabField.value = id;
  });
  function update() {
    if (!status) return;
    const current = new FormData(form);
    dirty = clean(current) !== original;
    const label = dirty ? form.dataset.dirtyLabel : form.dataset.cleanLabel;
    if (status.textContent !== label) status.textContent = label;
    form.classList.toggle('settings-dirty', dirty);
    for (const panel of panels) {
      const names = new Set([...panel.querySelectorAll('[name]')].map(el => el.name));
      const changed = clean([...current].filter(([name]) => names.has(name))) !== clean([...baseline].filter(([name]) => names.has(name)));
      const tab = tabFor(panel);
      if (tab) {
        tab.querySelector('[data-tab-dirty]').hidden = !changed;
        if (root.dataset.errorPanel !== panel.dataset.settingsPanel) {
          tab.querySelector('[data-tab-error]').hidden = ![...panel.querySelectorAll('[data-settings-invalid]')].some(el => !el.validity.valid);
        }
      }
    }
  }
  form.addEventListener('input', update);
  form.addEventListener('change', update);
  // Native validation must reveal a hidden invalid field before the browser focuses it.
  form.addEventListener('invalid', event => {
    const panel = event.target.closest('[data-settings-panel]');
    if (panel) {
      event.target.dataset.settingsInvalid = 'true';
      const tab = tabFor(panel);
      if (tab) tab.querySelector('[data-tab-error]').hidden = false;
      if (!form.querySelector('[data-invalid-first]')) {
        event.target.dataset.invalidFirst = 'true';
        setTimeout(() => { event.target.focus(); event.target.removeAttribute('data-invalid-first'); }, 0);
      }
    }
  }, true);
  form.addEventListener('submit', () => { submitting = true; });
  window.addEventListener('beforeunload', event => {
    if (dirty && !submitting) { event.preventDefault(); event.returnValue = ''; }
  });
  const discard = form.querySelector('[data-settings-discard]');
  if (discard) discard.addEventListener('click', event => {
    if (dirty && !window.confirm(form.dataset.discard)) { event.preventDefault(); return; }
    submitting = true;
  });
  window.addEventListener('pageshow', () => { submitting = false; update(); });
  const firstError = root.querySelector('[aria-invalid="true"]');
  if (root.dataset.errorPanel) {
    if (firstError) firstError.focus();
    else document.querySelector('#settings-error')?.focus();
  }
  update();
})();
