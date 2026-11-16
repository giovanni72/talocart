const assert = require('node:assert/strict');
module.exports = async page => {
  // A click may resolve before stylesheet loading; measure the completed document.
  await page.waitForLoadState('load');
  const failures = await page.evaluate(() => {
    const failures = [];
    if (document.body.textContent.includes('\u2197')) failures.push('Diagonal navigation arrow is no longer used');
    const visible = e => e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden';
    for (const control of document.querySelectorAll('button,input:not([type="hidden"]),select,textarea')) {
      const check = control.matches('input[type="checkbox"],input[type="radio"]');
      const expected = control.tagName === 'BUTTON' ? 'ui-button' : check ? 'ui-check' : 'ui-control';
      const name = control.id || control.name || control.tagName;
      if (!control.classList.contains(expected)) failures.push(`${name}: missing shared class`);
      if (!visible(control)) continue;
      if (control.tagName !== 'BUTTON' && !control.labels?.length && !control.getAttribute('aria-label')) failures.push(`${name}: missing label`);
      const folderNavigation = control.matches('button.ui-button--navigation.media-folder-root,button.ui-button--navigation.media-folder-link');
      // Rich text editor tools are a dense toolbar exception: 28px icon actions (30px on narrow screens).
      const richTool = control.matches('.ui-rich-text button.ui-rich-tool');
      const minimumHeight = folderNavigation ? (matchMedia('(max-width:600px)').matches ? 40 : 36) : richTool || control.matches('button.ui-button--mini') ? 28 : control.matches('button.ui-button--compact') ? 36 : 44;
      // Chromium's animated translations round float32 coordinates (e.g. 43.999996px for a 44px box).
      // Retain the full CSS size requirement and tolerate only coordinate roundoff.
      const measuredHeight = control.getBoundingClientRect().height;
      const coordinateRoundoff = measuredHeight >= minimumHeight - 0.0001 && parseFloat(getComputedStyle(control).height) >= minimumHeight;
      if (!check && measuredHeight < minimumHeight && !coordinateRoundoff) failures.push(`${name} (${control.className}, measured ${control.getBoundingClientRect().height}px): target below ${minimumHeight}px`);
      if (control.getAttribute('aria-invalid') === 'true') {
        const ids = (control.getAttribute('aria-describedby') || '').split(/\s+/).filter(Boolean);
        if (!ids.length || ids.some(id => !document.getElementById(id))) failures.push(`${name}: missing associated error`);
      }
    }
    for (const button of document.querySelectorAll('.ui-button')) {
      if (!visible(button)) continue;
      const labelledBy=(button.getAttribute('aria-labelledby')||'').split(/\s+/).filter(Boolean).map(id=>document.getElementById(id)?.textContent||'').join(' ').trim();
      if (!(button.getAttribute('aria-label')||'').trim() && !labelledBy && !button.textContent.trim()) failures.push('Button/link has no accessible name');
      for (const icon of button.querySelectorAll('svg[data-ui-icon]')) {
        if (!icon.classList.contains('ui-icon') || icon.getAttribute('aria-hidden')!=='true' || icon.getAttribute('focusable')!=='false') failures.push('Action icon must use the shared decorative SVG contract');
      }
      const style = getComputedStyle(button), ring = getComputedStyle(button,'::after');
      if (button.closest('#panel-image') && (!button.classList.contains('ui-button--compact') || style.minHeight !== '36px' || style.paddingTop !== '4px' || style.paddingBottom !== '4px')) failures.push('Image tab actions must use the shared compact button');
      if (button.classList.contains('ui-button--navigation')) {
        // Navigation uses native form buttons styled as links; selected folders
        // intentionally retain their own background and inset current marker.
        if (parseFloat(style.borderTopWidth) !== 0 || ring.content !== 'none') failures.push('Navigation control must not have action-button borders');
      } else if (button.matches('.ui-rich-text .ui-rich-tool')) {
        // Editor tools keep a background for hover, pressed and selected view states.
        if (parseFloat(style.borderTopWidth) !== 0 || ring.content !== 'none') failures.push('Rich text tool must have no border or decorative ring');
      } else if (button.classList.contains('ui-button--plain')) {
        if (parseFloat(style.borderTopWidth) !== 0 || style.backgroundColor !== 'rgba(0, 0, 0, 0)' || ring.content !== 'none') failures.push('Plain icon action must have no border, background or decorative ring');
      } else if (parseFloat(style.borderTopWidth) !== 2 || parseFloat(ring.borderTopWidth) !== 1 || parseFloat(ring.top) > -4 || ring.pointerEvents !== 'none') failures.push(`${button.id || button.textContent.trim()}: missing separated double border`);
      if (button.classList.contains('pagination-button')) {
        for (const node of button.childNodes) {
          if (node.nodeType !== Node.TEXT_NODE || !node.textContent.trim()) continue;
          const range = document.createRange(); range.selectNodeContents(node);
          if (range.getBoundingClientRect().height > parseFloat(style.lineHeight) + 1) failures.push('Pagination label wraps inside its button');
        }
      }
      const help = button.nextElementSibling;
      if (help?.classList.contains('ui-help') && visible(help) && help.getBoundingClientRect().top < button.getBoundingClientRect().bottom + 4) failures.push('Help text overlaps or crowds its button');
    }
    for (const message of document.querySelectorAll('.ui-message,.form-error,.form-success,.notice,.variant-selection-message,.status-content')) {
      if (!message.classList.contains('ui-message')) failures.push('Unshared message');
      if (visible(message) && parseFloat(getComputedStyle(message).borderInlineStartWidth) !== 3) failures.push('Message missing common border');
    }
    for (const empty of document.querySelectorAll('.catalog-empty,.store-empty,.media-empty,.ui-table__empty')) {
      if (!empty.classList.contains('ui-empty')) failures.push('Unshared empty state');
    }
    return failures;
  });
  assert.deepEqual(failures, [], `Shared UI contract: ${new URL(page.url()).pathname}`);
};
