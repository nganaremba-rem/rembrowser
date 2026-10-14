/*rembrowser:2*/
// Tabs here are not in any browser window, so extensions see them wrongly. The shim runs
// first in each extension's service worker (see extensions.rs).
(() => {
  // sender.tab.windowId is -1, which every chrome.windows call rejects: read it as "the
  // current window".
  if (chrome.windows) for (const m of ['get', 'update', 'remove']) {
    const orig = chrome.windows[m].bind(chrome.windows);
    chrome.windows[m] = (id, ...rest) => orig(id === -1 ? chrome.windows.WINDOW_ID_CURRENT : id, ...rest);
  }
  // "The active tab of the current window" is the hidden anchor window's about:blank, so
  // password managers find no logins for the page. Answer with the tab that last spoke to
  // us (a content script on the page being used).
  // ponytail: a background tab that messages first can win until the page in use messages
  // again; the browser would have to tell us the active tab to be exact.
  let last;
  const remember = (s) => { if (s && s.tab && s.tab.windowId === -1) last = s.tab; };
  chrome.runtime.onMessage.addListener((_m, s) => { remember(s); });
  chrome.runtime.onConnect.addListener((p) => remember(p.sender));
  const keys = ['active', 'currentWindow', 'lastFocusedWindow', 'windowId'];
  const query = chrome.tabs.query.bind(chrome.tabs);
  chrome.tabs.query = (info, cb) => {
    const wantsActive = info && info.active &&
      Object.keys(info).every((k) => keys.includes(k)) &&
      (info.currentWindow || info.lastFocusedWindow || info.windowId === chrome.windows.WINDOW_ID_CURRENT);
    const p = query(info).then((r) => (wantsActive && last ? [{ ...last, active: true }] : r));
    return cb ? void p.then(cb) : p;
  };
})();
/*end rembrowser*/
