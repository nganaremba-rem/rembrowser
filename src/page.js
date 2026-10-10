// rembrowser page helpers. Prepended to every command (`window.__rem ??=` makes it a
// no-op after the first run), so commands never depend on load timing.
// Replies go to Rust as console messages: "\x01rem\t<type>\t<field>...".
window.__rem ??= (() => {
  const send = (...f) => console.debug('\x01rem\t' + f.map((x) => String(x ?? '').replace(/[\t\n]/g, ' ')).join('\t'));

  // ── focus → insert-mode indicator ─────────────────────────────
  const deepActive = () => {
    let a = document.activeElement;
    while (a?.shadowRoot?.activeElement) a = a.shadowRoot.activeElement;
    return a;
  };
  const editable = (el) =>
    !!el && (el.isContentEditable ||
      (el.tagName === 'INPUT' && !/^(button|submit|reset|checkbox|radio|range|color|file|image|hidden)$/i.test(el.type)) ||
      el.tagName === 'TEXTAREA' || el.tagName === 'SELECT');
  const report = () => send('focus', editable(deepActive()) ? 1 : 0);
  addEventListener('focusin', report, true);
  addEventListener('focusout', () => setTimeout(report), true);

  // ── scrolling: the element the user is looking at, not always <html> ──
  const canScroll = (el, axis) => {
    const s = getComputedStyle(el);
    const ov = axis === 'y' ? s.overflowY : s.overflowX;
    const big = axis === 'y' ? el.scrollHeight > el.clientHeight + 4 : el.scrollWidth > el.clientWidth + 4;
    return big && /(auto|scroll|overlay)/.test(ov);
  };
  const scroller = (axis) => {
    const root = document.scrollingElement || document.documentElement;
    const rootBig = axis === 'y' ? root.scrollHeight > innerHeight + 4 : root.scrollWidth > innerWidth + 4;
    // Walk up from the focused element, then from the viewport centre.
    for (let el of [deepActive(), document.elementFromPoint(innerWidth / 2, innerHeight / 2)]) {
      for (; el && el !== document.body && el !== root; el = el.parentElement || el.getRootNode()?.host)
        if (el.nodeType === 1 && canScroll(el, axis)) return el;
    }
    if (rootBig) return root;
    // App-style pages (Gmail, chats): the biggest scrollable box.
    let best = root, area = 0;
    for (const el of document.querySelectorAll('*')) {
      if (!canScroll(el, axis)) continue;
      const r = el.getBoundingClientRect(), a = r.width * r.height;
      if (a > area) { area = a; best = el; }
    }
    return best;
  };
  const R = {};
  R.smooth = true;
  // kind: by (dx,dy px) | page (dy pages) | top | bottom | left | right | percent (dy)
  R.scroll = (kind, dx = 0, dy = 0) => {
    const el = scroller(kind === 'left' || kind === 'right' || dx ? 'x' : 'y');
    const behavior = R.smooth ? 'smooth' : 'instant';
    const h = el === document.scrollingElement ? innerHeight : el.clientHeight;
    switch (kind) {
      case 'by': el.scrollBy({ left: dx, top: dy, behavior }); break;
      case 'page': el.scrollBy({ top: dy * h, behavior }); break;
      case 'top': el.scrollTo({ top: 0, behavior }); break;
      case 'bottom': el.scrollTo({ top: el.scrollHeight, behavior }); break;
      case 'left': el.scrollTo({ left: 0, behavior }); break;
      case 'right': el.scrollTo({ left: el.scrollWidth, behavior }); break;
      case 'percent': el.scrollTo({ top: (el.scrollHeight - h) * dy / 100, behavior }); break;
    }
  };

  // ── hints ──────────────────────────────────────────────────────
  const CLICKABLE = 'a[href],area[href],button,input:not([type=hidden]),select,textarea,summary,label[for],' +
    '[role=button],[role=link],[role=tab],[role=checkbox],[role=radio],[role=menuitem],[role=option],[role=switch],' +
    '[role=treeitem],[onclick],[contenteditable=""],[contenteditable=true],[tabindex]:not([tabindex="-1"]),video,audio';
  const EDITABLE = 'input:not([type=hidden]),textarea,[contenteditable=""],[contenteditable=true]';
  // vimb hint modes: what each kind of hint may target.
  const FILTERS = { all: CLICKABLE, links: 'a[href],area[href]', images: 'img', editable: EDITABLE,
    media: 'a[href],area[href],video,audio' };
  const all = (sel, root = document, out = []) => {
    for (const el of root.querySelectorAll(sel)) out.push(el);
    for (const el of root.querySelectorAll('*')) if (el.shadowRoot) all(sel, el.shadowRoot, out);
    return out;
  };
  const visibleRect = (el) => {
    for (const r of el.getClientRects()) {
      if (r.width < 3 || r.height < 3 || r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) continue;
      const x = Math.min(Math.max(r.left + r.width / 2, 0), innerWidth - 1);
      const y = Math.min(Math.max(r.top + r.height / 2, 0), innerHeight - 1);
      // Skip elements covered by something else (modals, sticky headers).
      const top = (el.getRootNode().elementFromPoint?.(x, y)) ?? document.elementFromPoint(x, y);
      if (top && top !== el && !el.contains(top) && !top.contains(el)) continue;
      return { r, x, y };
    }
    return null;
  };
  // Same-length labels (vimb hint-keys-same-length): no label is a prefix of another.
  const labels = (n, keys) => {
    if (n <= 0) return [];
    const k = keys.length;
    let len = 1;
    while (k ** len < n) len++;
    const out = [];
    for (let i = 0; i < n; i++) {
      let s = '', v = i;
      for (let j = 0; j < len; j++) { s = keys[v % k] + s; v = Math.floor(v / k); }
      out.push(s);
    }
    return out;
  };
  // Adopted sheet, not innerHTML/<style>: YouTube & Google enforce Trusted Types (an
  // innerHTML assignment throws there) and CSP can block inline styles.
  const hintCss = new CSSStyleSheet();
  hintCss.replaceSync(`b{position:fixed;font:bold 12px/1 "CaskaydiaMono Nerd Font",monospace;padding:2px 4px;border-radius:4px;
    background:#f9e2af;color:#11111b;box-shadow:0 2px 6px #0008;text-transform:uppercase;letter-spacing:.5px}
    b i{font-style:normal;color:#7f6a2e} b.off{display:none}`);
  let H = null;
  R.hintsClear = () => { H?.host.remove(); H = null; };
  // A page that breaks hinting must not leave the browser stuck in HINT mode.
  R.hints = (keys, filter) => {
    try { hints(keys, filter); } catch (e) { R.hintsClear(); send('hint-none'); console.error(e); }
  };
  const hints = (keys, filter) => {
    R.hintsClear();
    const seen = new Set(), items = [];
    for (const el of all(FILTERS[filter] ?? CLICKABLE)) {
      if (seen.has(el) || (filter === 'editable' && !editable(el))) continue;
      seen.add(el);
      const v = visibleRect(el);
      if (v) items.push({ el, ...v });
    }
    if (!items.length) { send('hint-none'); return; }
    const host = document.createElement('rem-hints');
    host.style.cssText = 'all:initial;position:fixed;inset:0;z-index:2147483647;pointer-events:none';
    const sh = host.attachShadow({ mode: 'closed' });
    sh.adoptedStyleSheets = [hintCss];
    const ls = labels(items.length, keys);
    items.forEach((it, n) => {
      const b = document.createElement('b');
      b.textContent = ls[n];
      b.style.left = Math.max(0, it.r.left) + 'px';
      b.style.top = Math.max(0, it.r.top) + 'px';
      sh.append(b);
      it.label = ls[n];
      it.b = b;
    });
    document.documentElement.append(host);
    H = { host, items, typed: '' };
    send('hint-shown', items.length);
  };
  // Returns via console: hint-pick (chosen) or hint-none (nothing left).
  R.hintKey = (k) => {
    if (!H) return;
    H.typed = k === '<BS>' ? H.typed.slice(0, -1) : H.typed + k;
    const left = H.items.filter((it) => it.label.startsWith(H.typed));
    for (const it of H.items) {
      const on = it.label.startsWith(H.typed);
      it.b.className = on ? '' : 'off';
      if (on) {
        const i = document.createElement('i');
        i.textContent = H.typed;
        it.b.replaceChildren(i, it.label.slice(H.typed.length));
      }
    }
    if (!left.length) { R.hintsClear(); send('hint-none'); return; }
    if (left.length === 1 && left[0].label === H.typed) {
      const { el, x, y } = left[0];
      R.hintsClear();
      R.picked = el;  // for follow-ups: ;k removes it, ;e edits it
      const href = el.closest?.('a[href],area[href]')?.href || el.src || el.currentSrc || '';
      const text = (el.innerText || el.value || el.alt || el.title || '').trim().slice(0, 300);
      send('hint-pick', href, text, x, y, editable(el) ? 1 : 0);
    }
  };

  R.focusInput = () => {
    const el = all(EDITABLE).find((e) => editable(e) && visibleRect(e));
    el?.focus();
  };
  R.blur = () => deepActive()?.blur();

  // ── permission prompt inside a web-app window (no command popup there) ──
  R.ask = (id, origin, what) => {
    const host = document.createElement('rem-ask');
    host.style.cssText = 'all:initial;position:fixed;top:12px;left:50%;translate:-50% 0;z-index:2147483647';
    const sh = host.attachShadow({ mode: 'closed' });
    const css = new CSSStyleSheet();
    css.replaceSync(`div{font:14px/1.4 system-ui,sans-serif;color:#cdd6f4;background:#1e1e2e;border:1.5px solid #fab387;
      border-radius:12px;padding:12px 14px;box-shadow:0 12px 40px #000a;display:flex;gap:10px;align-items:center;flex-wrap:wrap}
      b{color:#fab387} button{font:inherit;color:#cdd6f4;background:#313244;border:1px solid #45475a;border-radius:8px;
      padding:4px 10px;cursor:pointer} button:hover{border-color:#fab387}`);
    sh.adoptedStyleSheets = [css];
    const box = document.createElement('div');
    const msg = document.createElement('span');
    const o = document.createElement('b'); o.textContent = origin;
    msg.append(o, ` wants to use your ${what}.`);
    box.append(msg);
    for (const [label, choice] of [['Allow once', 'allow'], ['Always allow', 'always'], ['Deny', 'deny'], ['Never', 'never']]) {
      const b = document.createElement('button');
      b.textContent = label;
      b.onclick = () => { host.remove(); send('perm-answer', id, choice); };
      box.append(b);
    }
    sh.append(box);
    document.documentElement.append(host);
  };

  // ── external editor: insert-mode Ctrl-T / ;e ──────────────────
  // The field is remembered: focus moves to the editor window meanwhile.
  R.editGet = (el = deepActive()) => {
    if (!editable(el) || el.tagName === 'SELECT') { send('edit-none'); return; }
    R.editing = el;
    send('edit-text', encodeURIComponent(el.isContentEditable ? el.innerText : el.value));
  };
  // insertText = a real edit: React/Vue/Lexical see input events, undo works.
  R.editSet = (t) => {
    const el = R.editing;
    if (!el?.isConnected) return;
    el.focus();
    if (el.isContentEditable) getSelection().selectAllChildren(el); else el.select();
    if (document.execCommand('insertText', false, t)) return;
    if (el.isContentEditable) { el.innerText = t; return; }
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value').set.call(el, t);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };

  // ── cosmetic adblock: CSS from adblock-rust (Rust side), adopted so CSP can't block it.
  // Generic `##.ad` rules only match names that exist, so we report class/id names
  // (once each) and Rust answers with the matching selectors.
  const sheet = new CSSStyleSheet();
  const seen = new Set();
  let rules = '', generic = false, cls = [], ids = [], timer = 0;
  const flush = () => { timer = 0; send('cls', cls.join(' '), ids.join(' ')); cls = []; ids = []; };
  const note = (el) => {
    for (const c of el.classList ?? []) if (!seen.has('.' + c)) seen.add('.' + c), cls.push(c);
    if (el.id && !seen.has('#' + el.id)) seen.add('#' + el.id), ids.push(el.id);
  };
  const collect = (root) => {
    if (root.nodeType !== 1 && root !== document) return;
    if (root.nodeType === 1) note(root);
    for (const el of root.querySelectorAll('[class],[id]')) note(el);
    if ((cls.length || ids.length) && !timer) timer = setTimeout(flush, 50);
  };
  R.cosmetic = (css, gen) => {
    if (css) {
      rules += css;
      sheet.replaceSync(rules);
      if (!document.adoptedStyleSheets.includes(sheet)) document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
    if (gen && !generic) {
      generic = true;
      const start = () => {
        collect(document);
        new MutationObserver((ms) => { for (const m of ms) for (const n of m.addedNodes) collect(n); })
          .observe(document.documentElement, { childList: true, subtree: true });
      };
      document.readyState === 'loading' ? addEventListener('DOMContentLoaded', start, { once: true }) : start();
    }
  };
  return R;
})();
