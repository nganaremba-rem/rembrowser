(async()=>{
 await new Promise((ok,no)=>{const s=document.createElement('script');s.src='https://cdnjs.cloudflare.com/ajax/libs/shaka-player/4.11.7/shaka-player.compiled.js';s.onload=ok;s.onerror=no;document.head.append(s)});
 const v=document.createElement('video');v.muted=true;v.width=640;document.body.append(v);
 const p=new shaka.Player();await p.attach(v);
 p.configure({drm:{servers:{'com.widevine.alpha':'https://cwip-shaka-proxy.appspot.com/no_auth'}}});
 const errs=[];p.addEventListener('error',e=>errs.push(e.detail.code));
 await p.load('https://storage.googleapis.com/shaka-demo-assets/angel-one-widevine/dash.mpd');
 await v.play();const t0=v.currentTime;await new Promise(r=>setTimeout(r,6000));
 const tr=p.getVariantTracks().find(t=>t.active);
 return `drm=${p.drmInfo()?.keySystem} t=${t0.toFixed(1)}→${v.currentTime.toFixed(1)} codec=${tr?.videoCodec}/${tr?.audioCodec} ${tr?.width}x${tr?.height} errs=${errs.join(',')||'none'} dropped=${v.getVideoPlaybackQuality().droppedVideoFrames}`;
})().catch(e=>'FAIL '+(e.code||e.message))
