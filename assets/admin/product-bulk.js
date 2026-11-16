// Native checkboxes/radios and server validation also work without JavaScript.
(() => {
  const actions = [...document.querySelectorAll('[data-bulk-action]')];
  const panel = document.querySelector('[data-bulk-category]');
  if (!actions.length || !panel) return;
  const radios = [...panel.querySelectorAll('input[type="radio"]')];
  const sync = () => {
    const assigning = actions.some(action => action.value === 'category' && action.checked);
    panel.hidden = !assigning;
    for (const radio of radios) {
      radio.disabled = !assigning;
      radio.required = assigning;
    }
  };
  for (const action of actions) action.addEventListener('change', () => {
    if (action.checked) for (const other of actions) {
      if (other === action) continue;
      if (action.value === 'delete' || other.value === 'delete' ||
          (['publish','draft'].includes(action.value) && ['publish','draft'].includes(other.value))) other.checked = false;
    }
    sync();
  });
  window.addEventListener('pageshow', sync);
  sync();
})();
