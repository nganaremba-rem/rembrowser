(async()=>{const ks='com.widevine.alpha';const tries={
 vp9:[{initDataTypes:['cenc','webm'],videoCapabilities:[{contentType:'video/webm; codecs="vp9"'}]}],
 h264:[{initDataTypes:['cenc'],videoCapabilities:[{contentType:'video/mp4; codecs="avc1.42E01E"'}]}],
 aac:[{initDataTypes:['cenc'],audioCapabilities:[{contentType:'audio/mp4; codecs="mp4a.40.2"'}]}],
 opus:[{initDataTypes:['cenc','webm'],audioCapabilities:[{contentType:'audio/webm; codecs="opus"'}]}]};
const r=[];for(const [k,c] of Object.entries(tries)){try{const a=await navigator.requestMediaKeySystemAccess(ks,c);await a.createMediaKeys();r.push(k+'=ok')}catch(e){r.push(k+'=no')}}
r.push('mse-h264='+MediaSource.isTypeSupported('video/mp4; codecs="avc1.42E01E"'));r.push('mse-aac='+MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'));r.push(navigator.userAgent.match(/Chrome\/[\d.]+/)[0]);return r.join(' ')})()
