// Usage: node cdp.mjs <url> <js-expression>  — evaluates in a page and prints the result.
// Default: navigates rembrowser's own first tab (so our handlers apply). NEW_TARGET=1 opens
// a DevTools-created target instead (CEF gives those no client: no adblock, no keys).
const [url, expr] = process.argv.slice(2);
const base = `http://127.0.0.1:${process.env.CDP_PORT ?? 9222}`;
const t = process.env.NEW_TARGET
  ? await (await fetch(`${base}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' })).json()
  : (await (await fetch(`${base}/json/list`)).json()).find((x) => x.type === 'page');
const ws = new WebSocket(t.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0;
const call = (method, params = {}) => new Promise((res) => {
  const my = ++id;
  const on = (e) => { const m = JSON.parse(e.data); if (m.id === my) { ws.removeEventListener('message', on); res(m); } };
  ws.addEventListener('message', on);
  ws.send(JSON.stringify({ id: my, method, params }));
});
if (!process.env.NEW_TARGET && !process.env.NO_NAV) await call('Page.navigate', { url });
await new Promise((r) => setTimeout(r, Number(process.env.WAIT ?? 3000)));
const r = await call('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
const v = r.result?.result?.value;
console.log(v !== undefined && typeof v !== 'object' ? String(v) : JSON.stringify(r.result ?? r.error));
ws.close();
