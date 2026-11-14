importScripts('shapes.js');
const boot = Date.now();
const events = { activated: 0, updated: 0 };
chrome.tabs.onActivated.addListener(() => events.activated++);
// Bitwarden's own rule (TabsBackground): once a real window had focus, an update from a tab
// of any other window is ignored, and its login list stays on the previous site.
let focusedWindowId = -1;
const bitwarden = { accepted: 0, ignored: 0, lastUrl: '' };
chrome.windows.onFocusChanged.addListener((id) => { if (id != null && id >= 0) focusedWindowId = id; });
chrome.tabs.onActivated.addListener(async () => {
  bitwarden.lastUrl = (await chrome.tabs.query({ active: true, windowId: chrome.windows.WINDOW_ID_CURRENT }))[0]?.url;
});
chrome.tabs.onUpdated.addListener(async (_id, _change, tab) => {
  events.updated++;
  if ((focusedWindowId > 0 && tab.windowId !== focusedWindowId) || !tab.active) return void bitwarden.ignored++;
  bitwarden.accepted++;
  bitwarden.lastUrl = (await chrome.tabs.query({ active: true, windowId: chrome.windows.WINDOW_ID_CURRENT }))[0]?.url;
});
let pop; // id of the window made by "win"
// Every "I am on screen / hidden" report from a page (see src/ext-active.js), newest last.
const said = [];
chrome.runtime.onMessage.addListener((m, s) => {
  if (m && m.__rembrowser) said.push(`${new URL(s.tab.url).host} ${m.__rembrowser}${m.focused ? ' focused' : ''}`);
});
const cmds = {
  // What the worker sees of the tab that asked, and of "the active tab".
  async bg(s) {
    const id = s.tab.id;
    return {
      boot, events, said: said.splice(0), focusedWindowId, bitwarden: { ...bitwarden },
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
