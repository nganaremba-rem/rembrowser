// Prints "type url" for every CDP target. Needs REMBROWSER_CDP set.
const { webSocketDebuggerUrl } = await (await fetch(`http://127.0.0.1:${process.env.CDP_PORT ?? 9222}/json/version`)).json();
const ws = new WebSocket(webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
ws.send(JSON.stringify({ id: 1, method: 'Target.getTargets' }));
const m = await new Promise((r) => ws.addEventListener('message', (e) => r(JSON.parse(e.data))));
for (const t of m.result.targetInfos) console.log(t.type, t.url.slice(0, 80));
ws.close();
