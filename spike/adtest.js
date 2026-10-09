(async()=>{const load=u=>new Promise(r=>{const s=document.createElement('script');s.src=u;s.onload=()=>r('LOADED');s.onerror=()=>r('blocked');document.head.append(s)});
const urls=['https://securepubads.g.doubleclick.net/tag/js/gpt.js','https://www.googletagmanager.com/gtag/js?id=G-XXXX','https://connect.facebook.net/en_US/fbevents.js','https://static.ads-twitter.com/uwt.js','https://cdnjs.cloudflare.com/ajax/libs/lodash.js/4.17.21/lodash.min.js'];
const r=[];for(const u of urls)r.push(new URL(u).host+'='+await load(u));return r.join(' ')})()
