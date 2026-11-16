// Order draft sections are shared ui-tabs panels (assets/ui-tabs.js). Without JavaScript every
// section stays visible; here the open section survives the submit and redirect of the form,
// and Enter inside a search field runs that search instead of the form's first button.
(() => {
  const form = document.querySelector('#order-editor-form');
  if (!form) return;
  // The page can also be rendered at another URL after a failed confirmation.
  const action = form.getAttribute('action');
  document.addEventListener('ui-tabs:change', event => {
    if (form.contains(event.target)) form.action = `${action}#${event.detail.panel.id}`;
  });
  form.addEventListener('keydown', event => {
    if (event.key !== 'Enter' || !event.target.matches('.oe-search-controls input')) return;
    event.preventDefault();
    event.target.parentElement.querySelector('button').click();
  });
})();
