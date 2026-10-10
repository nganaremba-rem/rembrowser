(async () => {
  const names = ['ad-banner', 'adsbygoogle', 'sponsored-post', 'totally-normal'];
  const els = names.map((n) => { const d = document.createElement('div'); d.className = n; d.textContent = n; document.body.append(d); return d; });
  await new Promise((r) => setTimeout(r, 600));
  return names.map((n, i) => n + '=' + getComputedStyle(els[i]).display).join(' ') + ' sheets=' + document.adoptedStyleSheets.length;
})()
