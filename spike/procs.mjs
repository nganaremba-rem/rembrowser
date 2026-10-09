// Prints "pid type" for every Chromium process (CDP SystemInfo.getProcessInfo). Needs REMBROWSER_CDP set.
const port = process.env.CDP_PORT ?? 9222;
const { webSocketDebuggerUrl } = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
const ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
ws.send(JSON.stringify({ id: 1, method: 'SystemInfo.getProcessInfo' }));
const m = await new Promise((r) => ws.addEventListener('message', (e) => r(JSON.parse(e.data))));
for (const p of m.result.processInfo) console.log(p.id, p.type);
ws.close();
