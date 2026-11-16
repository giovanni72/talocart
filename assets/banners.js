/* Progressive enhancement: first slide and destination work before initialization. */
(() => {
  'use strict';
  document.querySelectorAll('[data-banners]').forEach(root => {
    const slides = [...root.querySelectorAll('[data-banner]')];
    const controls = root.querySelector('[data-banner-controls]');
    if (slides.length < 2 || !controls) return;
    const dots = [...root.querySelectorAll('[data-banner-index]')];
    const toggle = root.querySelector('[data-banner-toggle]');
    const stage = root.querySelector('.home-banners-stage');
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
    let current = 0, playing = !reduced.matches, hovering = false, timer = null, gesture = null, swiped = false, toggleIntent = null;
    const clear = () => { if (timer !== null) window.clearTimeout(timer); timer = null; };
    const schedule = () => {
      clear();
      if (playing && !hovering && !document.hidden) timer = window.setTimeout(() => { show(current + 1); schedule(); }, 6000);
      toggle.textContent = playing ? root.dataset.pauseLabel : root.dataset.playLabel;
      stage.setAttribute('aria-live', playing ? 'off' : 'polite');
    };
    const show = index => {
      current = (index + slides.length) % slides.length;
      slides.forEach((slide, i) => { slide.hidden = i !== current; });
      dots.forEach((dot, i) => { if (i === current) dot.setAttribute('aria-current', 'true'); else dot.removeAttribute('aria-current'); });
    };
    const pause = () => { playing = false; schedule(); };
    const navigate = index => { pause(); show(index); };
    root.querySelector('[data-banner-prev]').addEventListener('click', () => navigate(current - 1));
    root.querySelector('[data-banner-next]').addEventListener('click', () => navigate(current + 1));
    dots.forEach((dot, i) => dot.addEventListener('click', () => navigate(i)));
    toggle.addEventListener('pointerdown', () => { toggleIntent = !playing; });
    toggle.addEventListener('pointercancel', () => { toggleIntent = null; });
    toggle.addEventListener('click', () => { playing = toggleIntent === null ? !playing : toggleIntent; toggleIntent = null; schedule(); });
    root.addEventListener('mouseenter', () => { hovering = true; schedule(); });
    root.addEventListener('mouseleave', () => { hovering = false; schedule(); });
    // Focus never starts rotation again automatically; Play is always an explicit decision.
    root.addEventListener('focusin', pause);
    document.addEventListener('visibilitychange', schedule);
    reduced.addEventListener('change', () => { if (reduced.matches) pause(); });
    root.addEventListener('keydown', event => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      navigate(event.key === 'Home' ? 0 : event.key === 'End' ? slides.length - 1 : current + (event.key === 'ArrowRight' ? 1 : -1));
      // Keep focus on the controls if the focused image link was hidden.
      if (slides.some(slide => slide.hidden && slide.contains(document.activeElement))) dots[current].focus();
    });
    stage.addEventListener('pointerdown', event => {
      swiped = false;
      gesture = event.isPrimary && event.pointerType !== 'mouse' ? { x: event.clientX, y: event.clientY, id: event.pointerId } : null;
    });
    stage.addEventListener('pointercancel', () => { gesture = null; swiped = false; });
    stage.addEventListener('pointerup', event => {
      if (!gesture || event.pointerId !== gesture.id) return;
      const dx = event.clientX - gesture.x, dy = event.clientY - gesture.y;
      gesture = null;
      if (Math.abs(dx) > 45 && Math.abs(dx) > Math.abs(dy) * 1.5) {
        swiped = true; navigate(current + (dx < 0 ? 1 : -1));
      }
    });
    stage.addEventListener('click', event => { if (swiped) { event.preventDefault(); swiped = false; } }, true);
    controls.hidden = false;
    schedule();
  });
})();
