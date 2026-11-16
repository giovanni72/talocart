// Native form association keeps selection out of the search query.
(() => {
  for (const table of document.querySelectorAll('[data-product-selection]')) {
    const all = table.querySelector('#product-select-all');
    const rows = [...table.querySelectorAll('.product-row-select')];
    if (!all) continue;
    const bulk = document.querySelector('#product-bulk-button');
    const sync = () => {
      const selected = rows.filter(row => row.checked).length;
      if (bulk) bulk.disabled = selected === 0;
      all.disabled = rows.length === 0;
      all.checked = rows.length > 0 && selected === rows.length;
      all.indeterminate = selected > 0 && selected < rows.length;
    };
    all.addEventListener('change', () => {
      for (const row of rows) row.checked = all.checked;
      sync();
    });
    for (const row of rows) row.addEventListener('change', sync);
    window.addEventListener('pageshow', sync);
    sync();
  }
})();
