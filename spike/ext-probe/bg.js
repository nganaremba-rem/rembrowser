importScripts('shapes.js');
const boot = Date.now();
const events = { activated: 0, updated: 0 };
chrome.tabs.onActivated.addListener(() => events.activated++);
chrome.tabs.onUpdated.addListener(() => events.updated++);
let pop; // id of the window made by "win"
const cmds = {
  // What the worker sees of the tab that asked, and of "the active tab".
  async bg(s) {
    const id = s.tab.id;
    return {
      boot, events,
      sender: [id, s.tab.windowId, s.tab.url],
      active: await askShapes(),
      get: await attempt(async () => (await chrome.tabs.get(id)).url),
      send: await attempt(() => chrome.tabs.sendMessage(id, { ping: 1 })),
      exec: await attempt(async () => (await chrome.scripting.executeScript({ target: { tabId: id }, func: () => location.host }))[0].result),
      popups: await attempt(async () => (await chrome.tabs.query({ windowType: 'popup' })).length),
      winMinus1: await attempt(async () => (await chrome.windows.get(-1)).type),
      winCurrent: await attempt(async () => (await chrome.windows.getCurrent()).type),
    };
  },
  // Bitwarden's "Unlock now": open a popout window.
  async win() {
    return attempt(async () => (pop = (await chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), type: 'popup', width: 380, height: 300 })).id));
  },
  // Bitwarden's second click: the popout exists, so it only asks to focus it.
  async focus() {
    return attempt(async () => (await chrome.windows.update(pop, { focused: true })).id);
  },
};
chrome.runtime.onMessage.addListener((m, s, reply) => {
  if (!cmds[m.probe]) return;
  cmds[m.probe](s).then(reply);
  return true;
});
