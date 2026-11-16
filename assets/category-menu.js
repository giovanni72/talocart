// Native disclosures remain usable without this progressive enhancement.
(() => {
  const navigation = document.querySelector('.category-navigation');
  if (!navigation) return;
  const desktop = window.matchMedia('(min-width: 801px)');
  const groups = [...navigation.querySelectorAll('.category-dropdown')];
  const closeGroups = () => groups.forEach(group => { group.open = false; });
  let isDesktop = desktop.matches;
  navigation.open = isDesktop;
  const resize = () => {
    // Some viewport operations emit a media event without changing the breakpoint.
    // Preserve the user's open panel until the layout actually changes.
    if (isDesktop === desktop.matches) return;
    isDesktop = desktop.matches;
    navigation.open = isDesktop;
    closeGroups();
  };
  desktop.addEventListener('change', resize);
  navigation.addEventListener('keydown', event => {
    if (event.key !== 'Escape') return;
    const group = event.target.closest('.category-dropdown[open]');
    if (group) {
      group.open = false;
      group.querySelector('summary').focus();
    } else if (!desktop.matches && navigation.open) {
      navigation.open = false;
      navigation.querySelector(':scope > summary').focus();
    }
  });
  document.addEventListener('click', event => {
    for (const group of groups) {
      if (group.open && !group.contains(event.target)) group.open = false;
    }
  });
})();
