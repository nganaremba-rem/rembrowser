// The page (driven over DevTools) asks through a DOM event; the answer lands in the DOM.
chrome.runtime.onMessage.addListener((m, _s, reply) => { if (m.ping) reply('pong ' + location.host); });
document.addEventListener('probe-cmd', async (e) => {
  let r;
  try { r = await chrome.runtime.sendMessage(JSON.parse(e.detail)); } catch (x) { r = 'ERR ' + x.message; }
  document.documentElement.dataset.cmd = JSON.stringify(r);
  document.dispatchEvent(new Event('probe-done'));
});
