// Native links, details and forms remain available without JavaScript.
(() => {
  const card = document.querySelector('.customer-card');
  if (!card) return;
  const forms = [...card.querySelectorAll('form')];
  const snapshot = form => JSON.stringify([...new FormData(form)].filter(([key]) => !['csrf', 'revision', 'action'].includes(key)));
  const original = new Map(forms.map(form => [form, form.dataset.caPending === 'true' ? null : snapshot(form)]));
  const dirty = form => snapshot(form) !== original.get(form);
  const status = card.querySelector('[data-ca-dirty-status]');
  const updateStatus = () => {
    if (status) status.textContent = forms.some(dirty) ? card.dataset.caDirty : '';
  };
  card.addEventListener('input', updateStatus);
  card.addEventListener('change', updateStatus);
  let leaving = false;
  // A full-page POST replaces every section, so protect other forms as well.
  for (const form of forms) form.addEventListener('submit', event => {
    if (forms.some(other => other !== form && dirty(other)) && !window.confirm(card.dataset.caDiscard)) {
      event.preventDefault();
      return;
    }
    leaving = true;
  });
  window.addEventListener('beforeunload', event => {
    if (!leaving && forms.some(dirty)) { event.preventDefault(); event.returnValue = ''; }
  });
  window.addEventListener('pageshow', () => { leaving = false; updateStatus(); });

  for (const form of card.querySelectorAll('#customer-profile-form, #customer-billing-form, #customer-address-form')) {
    const country = form.querySelector('[name=billing_country], [name=country]');
    const type = form.querySelector('[name=customer_type]');
    const refresh = () => {
      for (const field of form.querySelectorAll('[data-ca-field]')) {
        const input = field.querySelector('input, select');
        if (!input || input.closest('fieldset')?.disabled) continue;
        const applies = field.dataset.condition === 'region' ? ['IT', 'US', 'CA', 'AU'].includes(country?.value)
          : field.dataset.condition === 'it' ? country?.value === 'IT' : true;
        const required = type ? field.dataset[type.value === 'business' ? 'businessRequired' : 'privateRequired'] : field.dataset.configuredRequired;
        input.required = required === 'true' && applies;
        field.querySelector('[data-ca-required-mark]').hidden = !input.required;
      }
    };
    country?.addEventListener('change', refresh);
    type?.addEventListener('change', refresh);
    refresh();
    form.querySelector('button[value=refresh]')?.setAttribute('hidden', '');
  }

  // Sections are shared ui-tabs panels; ui-tabs.js opens the one holding the first error.
  const invalid = card.querySelector('[aria-invalid=true]');
  if (invalid) {
    const details = invalid.closest('details');
    if (details) details.open = true;
    invalid.focus();
  }
})();
