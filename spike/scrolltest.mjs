// Scroll smoothness check: "hold" a key (RATE repeats/s for MS) in the iso.sh instance
// and watch the page per frame. Usage: node scrolltest.mjs [key=j]   (window must be visible)
// PASS = the page moved in every frame while the key was held, and never jumped.
import net from 'node:net';
const key = process.argv[2] ?? 'j';
const rate = Number(process.env.RATE ?? 30), ms = Number(process.env.MS ?? 2000);
const sock = `${process.env.XDG_RUNTIME_DIR}/rembrowser-iso/rembrowser.sock`;
const base = `http://127.0.0.1:${process.env.CDP_PORT ?? 9333}`;
const t = (await (await fetch(`${base}/json/list`)).json()).find((x) => x.type === 'page' && !x.url.startsWith('data:text/html;base64,'));
const ws = new WebSocket(t.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let id = 0;
const evaluate = (expression) => new Promise((res) => {
  const my = ++id;
  const on = (e) => { const m = JSON.parse(e.data); if (m.id === my) { ws.removeEventListener('message', on); res(m.result?.result?.value); } };
  ws.addEventListener('message', on);
  ws.send(JSON.stringify({ id: my, method: 'Runtime.evaluate', params: { expression, returnByValue: true } }));
});
const sleep = (n) => new Promise((r) => setTimeout(r, n));
await evaluate(`scrollTo({top: ${key === 'k' ? 'document.documentElement.scrollHeight' : 0}, behavior: 'instant'});
  window.__f = []; window.__on = true;
  (function f(t) { __f.push([t, scrollY]); if (__on) requestAnimationFrame(f); })(performance.now()); 1`);
await sleep(300);
const end = Date.now() + ms;
while (Date.now() < end) { net.createConnection(sock).end(`cmd\tnormal ${key}\n`); await sleep(1000 / rate); }
await sleep(600);
const f = JSON.parse(await evaluate('__on = false; JSON.stringify(__f)'));
ws.close();
// Frames from the first movement to the last.
const d = f.slice(1).map(([t, y], i) => ({ dt: t - f[i][0], dy: Math.abs(y - f[i][1]) }));
const first = d.findIndex((x) => x.dy > 0), last = d.findLastIndex((x) => x.dy > 0);
const mov = d.slice(first, last + 1);
if (first < 0 || mov.length < 10) { console.log('FAIL: the page did not scroll'); process.exit(1); }
// Ignore the ease-in and ease-out: judge the cruise.
const cruise = mov.slice(Math.floor(mov.length * 0.15), Math.floor(mov.length * 0.85));
const speeds = cruise.map((x) => x.dy / x.dt * 1000);
const mean = speeds.reduce((a, b) => a + b, 0) / speeds.length;
const stalls = cruise.filter((x) => x.dy === 0).length;
const maxJump = Math.max(...mov.map((x) => x.dy));
const dev = Math.sqrt(speeds.reduce((a, b) => a + (b - mean) ** 2, 0) / speeds.length) / mean;
const total = mov.reduce((a, x) => a + x.dy, 0);
console.log(`frames ${mov.length}  total ${Math.round(total)}px  cruise ${Math.round(mean)}px/s  stalled frames ${stalls}  biggest frame step ${maxJump.toFixed(1)}px  speed spread ${(dev * 100).toFixed(0)}%`);
const ok = stalls <= cruise.length * 0.02 && maxJump < mean / 10 && dev < 0.35;
console.log(ok ? 'PASS' : 'FAIL');
process.exit(ok ? 0 : 1);
