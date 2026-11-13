// Same questions from the popup page; the answer is the window title.
(async () => {
  const active = await askShapes();
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const send = await attempt(() => chrome.tabs.sendMessage(tab.id, { ping: 1 }));
  document.title = 'POPUP ' + JSON.stringify({ active, send });
})();
