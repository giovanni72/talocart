import { Editor } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';

// Storage prefix matches platform::rich_text::PREFIX. The visual schema is
// deliberately narrower than the server policy: unsupported attributes stay in HTML.
const PREFIX = '<!--talocart-rich-text:v1-->';
const tags = new Set(['P', 'BR', 'H2', 'H3', 'STRONG', 'EM', 'U', 'S', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'A']);
const safeURL = value => /^(https?:\/\/|mailto:|tel:|\/(?!\/)|#)/i.test(value) && !/[\u0000-\u0020\u007f]/.test(value);
function supported(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  if (doc.head.children.length || /<!--|<!doctype|<\/?(?:html|head|body)(?:\s|>)/i.test(html)) return false;
  return [...doc.body.querySelectorAll('*')].every(node => tags.has(node.tagName) && [...node.attributes].every(attr =>
    node.tagName === 'A' && attr.name === 'href' && (attr.name !== 'href' || safeURL(attr.value))));
}
function plainDocument(text) {
  const escaped = text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  return escaped.replaceAll('\r\n', '\n').split('\n\n').map(part => '<p>' + part.replaceAll('\n', '<br>') + '</p>').join('');
}
function enhance(root) {
  if (root.dataset.richReady) return;
  const field = root.querySelector('textarea[name]');
  const format = root.querySelector('.ui-rich-native select');
  const enhancement = root.querySelector('.ui-rich-enhancement');
  const visual = root.querySelector('[data-rich-panel="visual"]');
  const html = root.querySelector('[data-rich-panel="html"]');
  const error = root.querySelector('[data-rich-error]');
  const buttons = [...root.querySelectorAll('[data-rich-view]')];
  const source = document.createElement('textarea');
  source.id = field.id + '-html'; source.className = 'ui-control ui-rich-source'; source.rows = 12;
  source.readOnly = field.readOnly; source.lang = field.lang; source.spellcheck = false;
  for (const attribute of ['aria-invalid', 'aria-describedby']) {
    if (field.hasAttribute(attribute)) source.setAttribute(attribute, field.getAttribute(attribute));
  }
  root.querySelector('.ui-rich-source-label').htmlFor = source.id;
  html.append(source);
  const label = document.querySelector('label[for="' + CSS.escape(field.id) + '"]');
  const labelText = label?.textContent || root.dataset.label;
  const surfaceId = field.id + '-visual';
  const initial = format.value === 'html' ? field.value : plainDocument(field.value);
  let mode = 'visual', loading = true, initialSupported = format.value !== 'html' || supported(initial);
  const maximum = Number(field.maxLength);
  const checkLength = () => {
    const valid = Array.from(field.value).length + (format.value === 'html' ? PREFIX.length : 0) <= maximum;
    if (!valid) error.textContent = root.dataset.limit;
    else if (error.textContent === root.dataset.limit) error.textContent = '';
    return valid;
  };
  const write = value => {
    field.value = value; format.value = 'html';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    field.dispatchEvent(new Event('change', { bubbles: true }));
    checkLength();
  };
  const syncToolbar = editor => {
    root.querySelectorAll('[data-rich-command]').forEach(button => {
      const key = button.dataset.richCommand;
      if (['bold', 'italic', 'underline', 'strike', 'paragraph', 'bulletList', 'orderedList', 'blockquote'].includes(key)) button.setAttribute('aria-pressed', String(editor.isActive(key)));
      if (key === 'heading2' || key === 'heading3') button.setAttribute('aria-pressed', String(editor.isActive('heading', { level: key === 'heading2' ? 2 : 3 })));
      if (key === 'undo') button.disabled = field.readOnly || !editor.can().undo();
      if (key === 'redo') button.disabled = field.readOnly || !editor.can().redo();
    });
  };
  let editor;
  try {
    editor = new Editor({
      element: root.querySelector('[data-rich-surface]'), injectCSS: false,
      coreExtensionOptions: { tabindex: { value: '0' } },
      editable: !field.readOnly, content: initial,
      extensions: [StarterKit.configure({
        heading: { levels: [2, 3] }, code: false, codeBlock: false, horizontalRule: false,
        link: { openOnClick: false, autolink: false, linkOnPaste: false, HTMLAttributes: { target: null, rel: null, class: null } },
      })],
      editorProps: { attributes: {
        id: surfaceId, role: 'textbox', 'aria-multiline': 'true', 'aria-label': labelText,
        lang: field.lang, 'aria-readonly': String(field.readOnly),
        ...(field.getAttribute('aria-describedby') ? { 'aria-describedby': field.getAttribute('aria-describedby') } : {}),
        ...(field.hasAttribute('aria-invalid') ? { 'aria-invalid': 'true' } : {}),
      } },
      onUpdate: ({ editor }) => { if (!loading) write(editor.getHTML()); },
      onTransaction: ({ editor }) => syncToolbar(editor),
    });
  } catch { source.remove(); return; } // Native control remains available on initialization failure.
  loading = false;
  syncToolbar(editor);
  buttons.forEach(button => {
    const key = button.dataset.richView;
    button.id = field.id + '-tab-' + key;
    button.setAttribute('aria-controls', field.id + '-panel-' + key);
    const panel = key === 'visual' ? visual : html;
    panel.id = field.id + '-panel-' + key;
    panel.setAttribute('aria-labelledby', button.id);
  });
  function switchMode(next, focus = false) {
    if (next === 'visual' && mode === 'html') {
      if (!supported(source.value)) { error.textContent = root.dataset.unsupported; source.focus(); return false; }
      loading = true;
      editor.commands.setContent(source.value, { emitUpdate: false });
      loading = false; error.textContent = '';
    }
    if (next === 'html' && mode !== 'html') source.value = format.value === 'html' ? field.value : editor.getHTML();
    mode = next;
    visual.hidden = next !== 'visual'; html.hidden = next !== 'html';
    root.querySelector('[data-rich-history]').hidden = next === 'html';
    root.querySelector('.ui-rich-link').hidden = true;
    buttons.forEach(button => { const active = button.dataset.richView === next; button.setAttribute('aria-selected', String(active)); button.tabIndex = active ? 0 : -1; });
    if (label) { if (next === 'visual') label.removeAttribute('for'); else label.htmlFor = source.id; }
    if (focus) buttons.find(button => button.dataset.richView === next).focus();
    return true;
  }
  buttons.forEach(button => {
    button.addEventListener('click', () => switchMode(button.dataset.richView));
    button.addEventListener('keydown', event => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault(); switchMode(event.key === 'Home' ? 'visual' : event.key === 'End' ? 'html' : button.dataset.richView === 'visual' ? 'html' : 'visual', true);
    });
  });
  source.addEventListener('input', () => { write(source.value); if (supported(source.value) && checkLength()) error.textContent = ''; });
  root.querySelectorAll('[data-rich-command]').forEach(button => button.addEventListener('click', () => {
    if (field.readOnly) return;
    const chain = editor.chain().focus();
    const actions = {
      bold: () => chain.toggleBold(), italic: () => chain.toggleItalic(), underline: () => chain.toggleUnderline(),
      strike: () => chain.toggleStrike(), paragraph: () => chain.setParagraph(),
      heading2: () => chain.toggleHeading({ level: 2 }), heading3: () => chain.toggleHeading({ level: 3 }),
      bulletList: () => chain.toggleBulletList(), orderedList: () => chain.toggleOrderedList(),
      blockquote: () => chain.toggleBlockquote(), undo: () => chain.undo(), redo: () => chain.redo(),
      clear: () => chain.unsetAllMarks().clearNodes(),
    };
    actions[button.dataset.richCommand]?.().run();
  }));
  const linkPanel = root.querySelector('.ui-rich-link');
  const linkInput = root.querySelector('[data-rich-link-url]');
  root.querySelector('[data-rich-link-open]').addEventListener('click', () => {
    if (field.readOnly) return;
    linkPanel.hidden = false; linkInput.value = editor.getAttributes('link').href || ''; linkInput.focus();
  });
  root.querySelector('[data-rich-link-apply]').addEventListener('click', () => {
    if (field.readOnly) return;
    const url = linkInput.value.trim();
    if (!safeURL(url)) { error.textContent = root.dataset.linkError; linkInput.focus(); return; }
    editor.chain().focus().extendMarkRange('link').setLink({ href: url }).run();
    linkPanel.hidden = true; error.textContent = '';
  });
  root.querySelector('[data-rich-link-remove]').addEventListener('click', () => {
    if (field.readOnly) return;
    editor.chain().focus().extendMarkRange('link').unsetLink().run(); linkPanel.hidden = true;
  });
  const closeLink = () => { linkPanel.hidden = true; editor.commands.focus(); };
  root.querySelector('[data-rich-link-close]').addEventListener('click', closeLink);
  linkInput.addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); root.querySelector('[data-rich-link-apply]').click(); } if (event.key === 'Escape') { event.preventDefault(); closeLink(); } });
  // The server owns validation and reopens the affected product tab on 422.
  field.form?.addEventListener('submit', checkLength);
  field.form?.addEventListener('reset', () => queueMicrotask(() => {
    loading = true; editor.commands.setContent(format.value === 'html' ? field.value : plainDocument(field.value), { emitUpdate: false }); loading = false;
    source.value = format.value === 'html' ? field.value : editor.getHTML(); error.textContent = '';
  }));
  root.dataset.richReady = 'true';
  field.hidden = true; root.querySelector('.ui-rich-native').hidden = true; root.querySelector('[data-rich-native-help]').hidden = true; enhancement.hidden = false;
  if (!initialSupported) { switchMode('html'); source.value = field.value; error.textContent = root.dataset.unsupported; }
  else if (label) label.removeAttribute('for');
  label?.addEventListener('click', event => { event.preventDefault(); if (mode === 'visual') editor.commands.focus(); else source.focus(); });
}
function initialize() { document.querySelectorAll('[data-rich-text]').forEach(enhance); }
initialize();
document.addEventListener('htmx:load', initialize);
