// The "which tab is active?" questions Bitwarden asks, in the worker and in its popup.
globalThis.SHAPES = {
  cur: { active: true, currentWindow: true },
  last: { active: true, lastFocusedWindow: true },
  wid: { active: true, windowId: -2 },
  bare: { active: true },
  hi: { active: true, highlighted: true },
};
globalThis.attempt = async (f) => { try { return await f(); } catch (e) { return 'ERR ' + e.message; } };
globalThis.askShapes = async () => {
  const r = {};
  for (const [k, q] of Object.entries(SHAPES)) r[k] = await attempt(async () => (await chrome.tabs.query(q)).map((t) => t.url).join(','));
  return r;
};
