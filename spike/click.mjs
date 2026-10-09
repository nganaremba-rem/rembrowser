// Usage: node click.mjs <x> <y> [button] — real mouse click (user gesture) in rembrowser's first tab.
const [x, y, button = 'left'] = process.argv.slice(2);
const t = (await (await fetch(`http://127.0.0.1:${process.env.CDP_PORT ?? 9222}/json/list`)).json()).find((p) => p.type === 'page');
const ws = new WebSocket(t.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0;
const send = (method, params) => new Promise((res) => {
  const my = ++id;
  ws.addEventListener('message', function on(e) { if (JSON.parse(e.data).id === my) { ws.removeEventListener('message', on); res(); } });
  ws.send(JSON.stringify({ id: my, method, params }));
});
for (const type of ['mousePressed', 'mouseReleased'])
  await send('Input.dispatchMouseEvent', { type, x: +x, y: +y, button, clickCount: 1 });
ws.close();
