
/*rembrowser:3*/
// Added to the end of an extension's own content script: the page tells the extension
// when it is the one on screen, and when it stops being so (ext-shim.js listens).
// pageshow = also a page that came back from the back/forward cache; navigatesuccess =
// a URL change without a load.
;(() => {
  if (window !== top) return;
  const say = (e) => {
    const visible = document.visibilityState === 'visible';
    if (!visible && !(e && e.type === 'visibilitychange')) return;
    const m = {
      __rembrowser: visible ? 'active' : 'hidden',
      focused: (e && e.type === 'focus') || document.hasFocus(),
      ready: document.readyState === 'complete',
    };
    try { chrome.runtime.sendMessage(m).catch(() => {}); } catch {}
  };
  addEventListener('focus', say);
  addEventListener('pageshow', say);
  document.addEventListener('visibilitychange', say);
  if (self.navigation) navigation.addEventListener('navigatesuccess', say);
  say();
})();
/*end rembrowser*/
