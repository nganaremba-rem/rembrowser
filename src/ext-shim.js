/*rembrowser:4*/
// Tabs here are not in any browser window, so extensions see them wrongly. This runs
// first in each extension's service worker and in its popup page (see extensions.rs).
// spike/ext-probe.sh shows every case below, before and after.
(() => {
  if (!chrome.tabs || !chrome.windows) return;
  const CUR = chrome.windows.WINDOW_ID_CURRENT;
  const win = {};
  for (const m of ['get', 'getAll', 'update', 'remove', 'create']) win[m] = chrome.windows[m].bind(chrome.windows);
  // Both call styles: f(args..., callback) and a promise.
  const reply = (p, cb) => (cb ? void p.then(cb) : p);
  const split = (args) => (typeof args[args.length - 1] === 'function' ? [args.slice(0, -1), args[args.length - 1]] : [args]);

  // sender.tab.windowId is -1, which every chrome.windows call rejects: read it as "the
  // current window" (the hidden anchor window, see tabs.rs).
  for (const m of ['get', 'remove']) {
    chrome.windows[m] = (id, ...rest) => win[m](id === -1 ? CUR : id, ...rest);
  }
  // "Focus my popout" (Bitwarden's Unlock when its window already exists): a background
  // worker cannot raise a window on Wayland, so nothing would happen while the popout
  // sits on another workspace. Open it again where the user is.
  const reopen = async (id) => {
    const w = await win.get(id, { populate: true });
    const url = w.tabs && w.tabs[0] && w.tabs[0].url;
    if (w.focused || w.type !== 'popup' || !url || !url.startsWith(chrome.runtime.getURL(''))) return;
    await win.remove(id);
    return win.create({ url, type: 'popup', width: w.width, height: w.height });
  };
  chrome.windows.update = (id, info, ...rest) => {
    const [, cb] = split([info, ...rest]);
    if (id === -1) id = CUR;
    const p = (info && info.focused && id !== CUR ? reopen(id).catch(() => {}) : Promise.resolve())
      .then((made) => made || win.update(id, info));
    return reply(p, cb);
  };

  // Our tabs have window id -1. Extensions that once saw a real window take focus (the
  // Unlock popout, the :extensions popup) then drop every event of a tab in another window:
  // Bitwarden kept the old site's logins after you moved on. So our tabs carry the hidden
  // anchor window's id (its only tab is about:blank), and focus moves there with them.
  let anchorId, lastFocus;
  const anchor = async () => {
    if (anchorId === undefined) {
      const all = await win.getAll({ populate: true }).catch(() => []);
      anchorId = (all.find((w) => w.tabs && w.tabs.length === 1 && w.tabs[0].url === 'about:blank') || {}).id;
    }
    return anchorId;
  };

  // The active tab. Chromium's answer is the anchor window's about:blank (or, in the
  // popup, the popup itself), so a password manager finds no site and lists everything.
  // The page in use says so itself (ext-active.js, added to the extension's content
  // script); kept in session storage because the worker is put to sleep and the popup
  // is another page.
  const KEY = '__rembrowser_active';
  const store = chrome.storage && chrome.storage.session;
  // active = the tab on screen; waiting = a visible tab that may take over (see below).
  let active, waiting, shown;
  const ready = store ? store.get(KEY).then((v) => { active = active || v[KEY]; }, () => {}) : Promise.resolve();
  const save = () => { if (store) (active ? store.set({ [KEY]: active }) : store.remove(KEY)).catch(() => {}); };
  // Chromium's own record of that tab: the URL of right now, and an error once it is gone.
  const current = async () => {
    await ready;
    while (active) {
      try {
        return { ...(await chrome.tabs.get(active.id)), windowId: active.windowId, active: true, highlighted: true };
      } catch {
        active = waiting; waiting = undefined; save();
      }
    }
  };

  // Our tabs never fire tabs.onActivated / onUpdated, which extensions use to refresh
  // what they show for "this page". Fire them when the page in use changes.
  const heard = { onActivated: chrome.tabs, onUpdated: chrome.tabs, onFocusChanged: chrome.windows };
  for (const [ev, api] of Object.entries(heard)) {
    const add = api[ev].addListener.bind(api[ev]);
    heard[ev] = [];
    api[ev].addListener = (f, ...rest) => { heard[ev].push(f); return add(f, ...rest); };
  }
  const fire = (ev, ...args) => heard[ev].forEach((f) => { try { f(...args); } catch (e) { console.error(e); } });
  chrome.windows.onFocusChanged.addListener((id) => { lastFocus = id; });
  const accept = async (tab, focused) => {
    const id = await anchor();
    if (id !== undefined) tab = { ...tab, windowId: id };
    const now = `${tab.id} ${tab.url} ${tab.status}`;
    const switched = !active || active.id !== tab.id;
    active = tab; save();
    if (focused && id !== undefined && lastFocus !== id) fire('onFocusChanged', id);
    if (now === shown) return;
    shown = now;
    if (switched) fire('onActivated', { tabId: tab.id, windowId: tab.windowId });
    fire('onUpdated', tab.id, { status: tab.status, url: tab.url }, tab);
  };
  // Two windows can each show a page. The one with keyboard focus wins; one without
  // takes over only when the active one went out of sight (tab switch) or is gone.
  chrome.runtime.onMessage.addListener((m, s) => {
    if (!m || !m.__rembrowser || !s.tab || s.tab.windowId !== -1) return;
    ready.then(() => {
      const id = s.tab.id;
      if (m.__rembrowser === 'hidden') {
        if (waiting && waiting.id === id) waiting = undefined;
        if (!active || active.id !== id) return;
        active.hidden = true; save();
        if (waiting) { accept(waiting); waiting = undefined; }
        return;
      }
      const tab = { ...s.tab, active: true, highlighted: true, status: m.ready ? 'complete' : 'loading' };
      if (m.focused || !active || active.hidden || active.id === id) accept(tab, m.focused);
      else waiting = tab;
    });
  });

  // Every "which tab is active?" question gets that tab. Other queries pass through.
  const query = chrome.tabs.query.bind(chrome.tabs);
  const asks = ['active', 'currentWindow', 'lastFocusedWindow', 'windowId', 'highlighted'];
  chrome.tabs.query = (info, cb) => {
    const wants = info && info.active === true && info.highlighted !== false &&
      Object.keys(info).every((k) => asks.includes(k)) &&
      (info.windowId === undefined || info.windowId === CUR || info.windowId === -1 ||
        (anchorId !== undefined && info.windowId === anchorId));
    if (!wants) return reply(query(info), cb);
    // No window named = the active tab of every window: ours first, never the anchor's.
    const everyWindow = !info.currentWindow && !info.lastFocusedWindow && info.windowId === undefined;
    const p = current().then(async (tab) => {
      const real = !tab || everyWindow ? await query(info.windowId === -1 ? { ...info, windowId: CUR } : info) : [];
      return tab ? [tab, ...real.filter((t) => t.url !== 'about:blank' && t.id !== tab.id)] : real;
    });
    return reply(p, cb);
  };
})();
/*end rembrowser*/
