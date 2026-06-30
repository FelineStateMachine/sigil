import {
  parseAnnexBNals,
  nalsToLengthPrefixed,
  buildAvcDescription,
  avcCodecStr,
  h264NalType,
  buildHvcDescription,
  hevcCodecStr,
  hevcNalType,
  parseAv1Obus,
  av1CodecStr,
  buildAv1Description,
} from './codecs.js';

let controlMode = false;
let connected = false;
let hosting = false;
let registered = false;
let frameWidth = 0;
let frameHeight = 0;

const invoke = (...args) => window.__TAURI__.core.invoke(...args);
const listen = (...args) => window.__TAURI__.event.listen(...args);

function updatePanelVisibility() {
  const encSection = document.getElementById('encoder-section');
  const hostPerfSection = document.getElementById('host-perf-section');
  const streamSection = document.getElementById('stream-section');
  encSection.style.display = connected ? 'none' : '';
  hostPerfSection.style.display = connected ? 'none' : '';
  streamSection.style.display = connected ? '' : 'none';
}

// ─── Status ──────────────────────────────────────────────────────────────────
function setStatus(state, text) {
  const line = document.getElementById('status-line');
  const dot = document.getElementById('status-dot');
  const txt = document.getElementById('status-text');
  line.className = 'status-line ' + state;
  dot.className = 'dot ' + state;
  txt.textContent = text;
}

function showIntro() {
  document.getElementById('intro').classList.remove('hidden');
  setTimeout(() => {
    const unregPin = document.getElementById('intro-pin-unreg');
    const regPin = document.getElementById('intro-pin-reg');
    const visible = unregPin.parentElement.parentElement.style.display !== 'none' ? unregPin : regPin;
    if (visible) visible.focus();
  }, 50);
}
function hideIntro() { document.getElementById('intro').classList.add('hidden'); }
function showTap() {
  document.getElementById('tap-overlay-title').textContent = 'tap key';
  document.getElementById('tap-overlay-desc').textContent = 'Touch the sensor on your key when it blinks.';
  document.getElementById('tap-status').textContent = 'waiting...';
  document.getElementById('tap-overlay').classList.remove('hidden');
}
function hideTap() { document.getElementById('tap-overlay').classList.add('hidden'); }

// ─── Registration ─────────────────────────────────────────────────────────────
async function checkRegistration() {
  try {
    const reg = await invoke('host_registration_status');
    registered = reg.registered;
    const unregDiv = document.getElementById('intro-unregistered');
    const regDiv = document.getElementById('intro-registered');
    if (reg.registered) {
      unregDiv.style.display = 'none';
      regDiv.style.display = 'block';
      document.getElementById('node-id-text').textContent = reg.node_id.substring(0, 16) + '...';
      document.getElementById('reg-status').textContent = 'registered';
      document.getElementById('reg-node-id').textContent = reg.node_id.substring(0, 24) + '...';
      document.getElementById('action-main').textContent = 'host';
      document.getElementById('action-unreg').style.display = '';
    } else {
      unregDiv.style.display = 'block';
      regDiv.style.display = 'none';
      document.getElementById('reg-status').textContent = 'client-only';
      document.getElementById('action-main').textContent = 'register';
      document.getElementById('action-unreg').style.display = 'none';
    }

    const daemon = await invoke('is_daemon_mode');
    if (daemon && reg.registered) {
      hideIntro();
      await startHost();
    } else {
      showIntro();
    }
  } catch (e) {
    console.error('registration check failed:', e);
    showIntro();
  }
}

// ─── Intro actions ────────────────────────────────────────────────────────────
async function introRegister() {
  const pin = document.getElementById('intro-pin-unreg').value.trim();
  const status = document.getElementById('intro-status-unreg');
  if (!pin) {
    status.className = 'overlay-status err';
    status.textContent = 'enter pin';
    return;
  }
  hideIntro();
  showTap();
  try {
    const result = await invoke('key_register_host', { pin });
    if (result.registered) {
      hideTap();
      registered = true;
      document.getElementById('node-id-text').textContent = result.node_id.substring(0, 16) + '...';
      document.getElementById('action-main').textContent = 'host';
      document.getElementById('action-unreg').style.display = '';
      document.getElementById('intro-unregistered').style.display = 'none';
      document.getElementById('intro-registered').style.display = 'block';
      document.getElementById('reg-status').textContent = 'registered';
      document.getElementById('reg-node-id').textContent = result.node_id.substring(0, 24) + '...';
      showConfigScreen();
    } else {
      hideTap();
      showIntro();
      status.className = 'overlay-status err';
      status.textContent = 'failed';
    }
  } catch (e) {
    hideTap();
    showIntro();
    status.className = 'overlay-status err';
    status.textContent = 'error: ' + e;
  }
}

async function introConnectUnreg() {
  const pin = document.getElementById('intro-pin-unreg').value.trim();
  const status = document.getElementById('intro-status-unreg');
  if (!pin) {
    status.className = 'overlay-status err';
    status.textContent = 'enter pin';
    return;
  }
  hideIntro();
  showTap();
  document.getElementById('pin-input').value = pin;
  await connectHost();
  hideTap();
  if (!connected) {
    showIntro();
    status.className = 'overlay-status err';
    status.textContent = 'connection failed';
  }
}

async function introConnectReg() {
  const pin = document.getElementById('intro-pin-reg').value.trim();
  const status = document.getElementById('intro-status-reg');
  if (!pin) {
    status.className = 'overlay-status err';
    status.textContent = 'enter pin';
    return;
  }
  hideIntro();
  showTap();
  document.getElementById('pin-input').value = pin;
  await connectHost();
  hideTap();
  if (!connected) {
    showIntro();
    status.className = 'overlay-status err';
    status.textContent = 'connection failed';
  }
}

async function introHost() {
  hideIntro();
  showConfigScreen();
}

// ─── Config screen (pre-host) ─────────────────────────────────────────────────
async function showConfigScreen() {
  try {
    const config = await invoke('get_encoder_config');
    document.getElementById('cfg-codec').value = config.codec;
    document.getElementById('cfg-backend').value = config.backend;
    document.getElementById('cfg-bitrate').value = config.bitrate;
    document.getElementById('cfg-framerate').value = config.framerate;
    document.getElementById('cfg-gop').value = config.gop;
  } catch (e) { console.error('config load failed:', e); }
  try {
    const available = await invoke('detect_available_encoders');
    document.getElementById('cfg-available').textContent = available.join(', ') || 'none';
  } catch (e) {
    document.getElementById('cfg-available').textContent = 'ffmpeg not found';
  }
  document.getElementById('config-status').textContent = '';
  document.getElementById('config-overlay').classList.remove('hidden');
}

function hideConfigScreen() {
  document.getElementById('config-overlay').classList.add('hidden');
}

function configBack() {
  hideConfigScreen();
  showIntro();
}

async function configHost() {
  const status = document.getElementById('config-status');
  status.className = 'overlay-status pending';
  status.textContent = 'saving...';
  try {
    const config = {
      codec: document.getElementById('cfg-codec').value,
      backend: document.getElementById('cfg-backend').value,
      bitrate: document.getElementById('cfg-bitrate').value,
      framerate: parseInt(document.getElementById('cfg-framerate').value) || 30,
      gop: parseInt(document.getElementById('cfg-gop').value) || 30,
    };
    await invoke('set_encoder_config', { config });
    hideConfigScreen();
    await startHost();
  } catch (e) {
    status.className = 'overlay-status err';
    status.textContent = 'error: ' + e;
  }
}

// ─── Host ─────────────────────────────────────────────────────────────────────
async function startHost() {
  setStatus('pending', 'starting...');
  try {
    const result = await invoke('iroh_host_start');
    if (result.running) {
      hosting = true;
      setStatus('ok', 'hosting');
      updatePanelVisibility();
      document.getElementById('node-id-text').textContent = result.node_id.substring(0, 16) + '...';
      document.getElementById('action-disconnect').textContent = 'stop hosting';
      document.getElementById('action-disconnect').classList.remove('hidden');
      document.getElementById('sep-disconnect-main').classList.remove('hidden');
      document.getElementById('action-main').style.display = 'none';
      document.getElementById('action-unreg').style.display = 'none';
      document.getElementById('placeholder').classList.add('hidden');
      document.getElementById('host-info').classList.remove('hidden');
      document.getElementById('host-node-id').textContent = result.node_id.substring(0, 24) + '...';
      document.getElementById('pin-label').classList.add('hidden');
      document.getElementById('pin-input').classList.add('hidden');
      document.getElementById('action-connect').style.display = 'none';
    } else {
      setStatus('err', 'failed');
    }
  } catch (e) {
    setStatus('err', 'error');
  }
}

async function stopHost() {
  try {
    await invoke('iroh_host_stop');
  } catch (e) { console.error(e); }
  hosting = false;
  setStatus('', 'idle');
  updatePanelVisibility();
  document.getElementById('action-main').style.display = '';
  document.getElementById('action-main').textContent = registered ? 'host' : 'register';
  document.getElementById('action-unreg').style.display = registered ? '' : 'none';
  document.getElementById('action-disconnect').classList.add('hidden');
  document.getElementById('sep-disconnect-main').classList.add('hidden');
  document.getElementById('host-info').classList.add('hidden');
  document.getElementById('placeholder').classList.remove('hidden');
  document.getElementById('pin-label').classList.remove('hidden');
  document.getElementById('pin-input').classList.remove('hidden');
  document.getElementById('action-connect').style.display = '';
}

// ─── Main action (host/register) ──────────────────────────────────────────────
async function mainAction() {
  if (hosting) return;
  if (registered) {
    showConfigScreen();
  } else {
    showIntro();
  }
}

// ─── Client ───────────────────────────────────────────────────────────────────
async function connectHost() {
  const pin = document.getElementById('pin-input').value.trim();
  if (!pin) {
    setStatus('err', 'enter pin');
    return;
  }
  setStatus('pending', 'connecting...');
  try {
    const result = await invoke('iroh_client_connect', { pin });
    if (result.connected) {
      connected = true;
      setStatus('ok', 'connected');
      droppedFrames = 0;
      receivedFrames = 0;
      updatePanelVisibility();
      document.getElementById('node-id-text').textContent = result.host_node_id.substring(0, 16) + '...';
      document.getElementById('frame-canvas').style.display = 'block';
      document.getElementById('placeholder').classList.add('hidden');
      document.getElementById('control-bar').classList.add('visible');
      document.getElementById('action-connect').style.display = 'none';
      document.getElementById('action-disconnect').textContent = 'disconnect';
      document.getElementById('action-disconnect').classList.remove('hidden');
      document.getElementById('sep-disconnect').classList.remove('hidden');
      document.getElementById('action-main').style.display = 'none';
    } else {
      setStatus('err', 'failed');
    }
  } catch (e) {
    setStatus('err', 'error');
  }
}

async function disconnect() {
  if (hosting) {
    await stopHost();
    return;
  }
  try {
    await invoke('iroh_client_disconnect');
  } catch (e) { console.error(e); }
  connected = false;
  controlMode = false;
  updateControlUI();
  updatePanelVisibility();
  setStatus('', 'idle');
  document.getElementById('frame-canvas').style.display = 'none';
  document.getElementById('placeholder').classList.remove('hidden');
  document.getElementById('placeholder').textContent = 'no stream';
  document.getElementById('stats').classList.remove('visible');
  document.getElementById('control-bar').classList.remove('visible');
  document.getElementById('action-connect').style.display = '';
  document.getElementById('action-disconnect').classList.add('hidden');
  document.getElementById('sep-disconnect').classList.add('hidden');
  document.getElementById('action-main').style.display = '';
  document.getElementById('intro-status-unreg').textContent = '';
  document.getElementById('intro-status-reg').textContent = '';
}

// ─── Unregister ───────────────────────────────────────────────────────────────
function unregister() {
  document.getElementById('action-unreg').style.display = 'none';
  document.getElementById('action-unreg-confirm').classList.remove('hidden');
  document.getElementById('action-unreg-cancel').classList.remove('hidden');
}

function unregisterCancel() {
  document.getElementById('action-unreg-confirm').classList.add('hidden');
  document.getElementById('action-unreg-cancel').classList.add('hidden');
  document.getElementById('action-unreg').style.display = '';
}

async function unregisterConfirm() {
  document.getElementById('action-unreg-confirm').classList.add('hidden');
  document.getElementById('action-unreg-cancel').classList.add('hidden');
  try {
    await invoke('host_unregister');
    registered = false;
    setStatus('', 'idle');
    document.getElementById('node-id-text').textContent = '';
    document.getElementById('reg-status').textContent = 'client-only';
    document.getElementById('reg-node-id').textContent = '—';
    document.getElementById('action-main').textContent = 'register';
    document.getElementById('action-unreg').style.display = 'none';
    showIntro();
  } catch (e) {
    alert('unregister failed: ' + e);
    document.getElementById('action-unreg').style.display = '';
  }
}

// ─── FIDO scan ────────────────────────────────────────────────────────────────
async function scanFido() {
  try {
    const info = await invoke('fido_device_info');
    const el = document.getElementById('fido-status');
    if (!info.found) {
      el.textContent = 'no device';
      el.style.color = 'var(--muted-fg)';
      return;
    }
    el.textContent = info.error ? `found (${info.error})` : 'connected';
    el.style.color = info.error ? 'var(--yellow)' : 'var(--green)';
    document.getElementById('fido-product').textContent = info.product || '—';
    document.getElementById('fido-vidpid').textContent =
      `${info.vid.toString(16).padStart(4,'0')}:${info.pid.toString(16).padStart(4,'0')}`;
    document.getElementById('fido-versions').textContent = info.versions.join(', ') || '—';
    document.getElementById('fido-extensions').textContent = info.extensions.join(', ') || '—';
    document.getElementById('fido-pin').textContent = info.pin_retries ?? '—';
  } catch (e) {
    console.error('scanFido failed:', e);
    document.getElementById('fido-status').textContent = 'error';
  }
}

// ─── Panel ────────────────────────────────────────────────────────────────────
function togglePanel() {
  document.getElementById('panel').classList.toggle('visible');
  document.getElementById('panel-overlay').classList.toggle('visible');
}

// ─── Host connection events ───────────────────────────────────────────────────
listen('host-connections', (event) => {
  document.getElementById('host-conn-count').textContent = event.payload;
});

// ─── Encode performance sparkline ─────────────────────────────────────────────
const encodeHistory = [];
const MAX_HISTORY = 60;
const sparkCanvas = document.getElementById('encode-spark');
const sparkCtx = sparkCanvas.getContext('2d');

function drawSparkline() {
  const w = sparkCanvas.width;
  const h = sparkCanvas.height;
  sparkCtx.clearRect(0, 0, w, h);
  if (encodeHistory.length < 2) return;

  const max = Math.max(...encodeHistory, 1);
  const step = w / (MAX_HISTORY - 1);

  sparkCtx.beginPath();
  sparkCtx.moveTo(0, h);
  encodeHistory.forEach((ms, i) => {
    const x = i * step;
    const y = h - (ms / max) * (h - 4) - 2;
    sparkCtx.lineTo(x, y);
  });
  sparkCtx.lineTo((encodeHistory.length - 1) * step, h);
  sparkCtx.closePath();
  sparkCtx.fillStyle = 'rgba(74, 222, 128, 0.15)';
  sparkCtx.fill();

  sparkCtx.beginPath();
  encodeHistory.forEach((ms, i) => {
    const x = i * step;
    const y = h - (ms / max) * (h - 4) - 2;
    if (i === 0) sparkCtx.moveTo(x, y);
    else sparkCtx.lineTo(x, y);
  });
  sparkCtx.strokeStyle = '#4ade80';
  sparkCtx.lineWidth = 1;
  sparkCtx.stroke();
}

listen('host-encode-stats', (event) => {
  const { encode_ms, capture_ms, size_bytes, fps, encoder } = event.payload;
  document.getElementById('perf-encode').textContent = encode_ms.toFixed(1) + ' ms';
  document.getElementById('perf-capture').textContent = encoder || (capture_ms > 0 ? capture_ms.toFixed(1) + ' ms' : 'ffmpeg');
  document.getElementById('perf-size').textContent = (size_bytes / 1024).toFixed(1) + ' KB';
  document.getElementById('perf-fps').textContent = fps.toFixed(1);

  encodeHistory.push(encode_ms);
  if (encodeHistory.length > MAX_HISTORY) encodeHistory.shift();
  drawSparkline();
});

// ─── WebCodecs detection ──────────────────────────────────────────────────────
let hasWebCodecs = ('VideoDecoder' in window);
let videoDecoder = null;
let decoderConfigured = false;

(async function detectWebCodecs() {
  await invoke('set_webcodecs_available', { available: hasWebCodecs });
})();

// ─── Encoder config ───────────────────────────────────────────────────────────
(async function loadEncoderConfig() {
  try {
    const config = await invoke('get_encoder_config');
    document.getElementById('enc-codec').value = config.codec;
    document.getElementById('enc-backend').value = config.backend;
    document.getElementById('enc-bitrate').value = config.bitrate;
    document.getElementById('enc-framerate').value = config.framerate;
    document.getElementById('enc-gop').value = config.gop;
  } catch (e) { console.error('encoder config load failed:', e); }

  try {
    const available = await invoke('detect_available_encoders');
    document.getElementById('enc-available').textContent = available.join(', ') || 'none';
  } catch (e) {
    document.getElementById('enc-available').textContent = 'ffmpeg not found';
  }
})();

async function saveEncoderConfig() {
  const config = {
    codec: document.getElementById('enc-codec').value,
    backend: document.getElementById('enc-backend').value,
    bitrate: document.getElementById('enc-bitrate').value,
    framerate: parseInt(document.getElementById('enc-framerate').value) || 30,
    gop: parseInt(document.getElementById('enc-gop').value) || 30,
  };
  await invoke('set_encoder_config', { config });
}

// ─── Frame decoding ───────────────────────────────────────────────────────────
let activeCodec = 'h264';
let droppedFrames = 0;
let receivedFrames = 0;

const canvas = document.getElementById('frame-canvas');
const ctx = canvas.getContext('2d');

function initWebCodecsDecoder(width, height, desc, codecStr) {
  if (videoDecoder) {
    try { videoDecoder.close(); } catch(e) {}
  }
  console.log('WebCodecs configure:', codecStr, 'w:', width, 'h:', height, 'desc:', desc.byteLength, 'bytes');
  videoDecoder = new VideoDecoder({
    output: (frame) => {
      ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);
      frame.close();
    },
    error: (e) => {
      console.error('VideoDecoder error:', e);
      decoderConfigured = false;
      try { videoDecoder.close(); } catch(_) {}
      videoDecoder = null;
    }
  });
  videoDecoder.configure({
    codec: codecStr,
    codedWidth: width,
    codedHeight: height,
    description: desc,
    optimizeForRealtimeUse: true,
  });
  decoderConfigured = true;
}

function updateStreamStats() {
  document.getElementById('stream-received').textContent = receivedFrames;
  document.getElementById('stream-dropped').textContent = droppedFrames;
  document.getElementById('stream-codec').textContent = activeCodec;
}

listen('frame', (event) => {
  const { width, height, data, codec } = event.payload;
  frameWidth = width;
  frameHeight = height;
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;

  if (codec && codec !== activeCodec) {
    activeCodec = codec;
    decoderConfigured = false;
    if (videoDecoder) {
      try { videoDecoder.close(); } catch(_) {}
      videoDecoder = null;
    }
    console.log('codec changed:', activeCodec);
  }

  receivedFrames++;

  if (hasWebCodecs) {
    const raw = Uint8Array.from(atob(data), c => c.charCodeAt(0));

    if (activeCodec === 'av1') {
      // ── AV1 path ──
      const obus = parseAv1Obus(raw);
      if (event.payload.keyframe) {
        const seqHeader = obus.find(o => o.type === 12);
        if (seqHeader && !decoderConfigured) {
          initWebCodecsDecoder(width, height, buildAv1Description(seqHeader.data), av1CodecStr(seqHeader.data));
        }
      }
      if (!decoderConfigured || !videoDecoder) { droppedFrames++; updateStreamStats(); return; }
      const frameObus = obus.filter(o => o.type !== 2 && o.type !== 12);
      if (frameObus.length > 0) {
        let totalLen = 0;
        for (const o of frameObus) totalLen += o.data.length;
        const chunkData = new Uint8Array(totalLen);
        let off = 0;
        for (const o of frameObus) { chunkData.set(o.data, off); off += o.data.length; }
        const chunk = new EncodedVideoChunk({
          type: event.payload.keyframe ? 'key' : 'delta',
          timestamp: performance.now() * 1000,
          data: chunkData,
        });
        try { videoDecoder.decode(chunk); } catch (e) { droppedFrames++; console.warn('av1 decode failed:', e); }
      }

    } else if (activeCodec === 'h265') {
      // ── H.265 path ──
      const nals = parseAnnexBNals(raw);
      if (event.payload.keyframe) {
        let vps = null, sps = null, pps = null;
        for (const nal of nals) {
          const t = hevcNalType(nal);
          if (t === 32) vps = nal;
          else if (t === 33) sps = nal;
          else if (t === 34) pps = nal;
        }
        if (vps && sps && pps && !decoderConfigured) {
          initWebCodecsDecoder(width, height, buildHvcDescription(vps, sps, pps), hevcCodecStr(sps));
        }
      }
      if (!decoderConfigured || !videoDecoder) { droppedFrames++; updateStreamStats(); return; }
      const sliceNals = nals.filter(nal => {
        const t = hevcNalType(nal);
        return t !== 32 && t !== 33 && t !== 34 && t !== 35;
      });
      if (sliceNals.length > 0) {
        const chunk = new EncodedVideoChunk({
          type: event.payload.keyframe ? 'key' : 'delta',
          timestamp: performance.now() * 1000,
          data: nalsToLengthPrefixed(sliceNals),
        });
        try { videoDecoder.decode(chunk); } catch (e) { droppedFrames++; console.warn('hevc decode failed:', e); }
      }

    } else {
      // ── H.264 path (default) ──
      const nals = parseAnnexBNals(raw);
      if (event.payload.keyframe) {
        let sps = null, pps = null;
        for (const nal of nals) {
          const t = h264NalType(nal);
          if (t === 7) sps = nal;
          else if (t === 8) pps = nal;
        }
        if (sps && pps && !decoderConfigured) {
          initWebCodecsDecoder(width, height, buildAvcDescription(sps, pps), avcCodecStr(sps));
        }
      }
      if (!decoderConfigured || !videoDecoder) { droppedFrames++; updateStreamStats(); return; }
      const sliceNals = nals.filter(nal => {
        const t = h264NalType(nal);
        return t !== 7 && t !== 8 && t !== 9;
      });
      if (sliceNals.length > 0) {
        const chunk = new EncodedVideoChunk({
          type: event.payload.keyframe ? 'key' : 'delta',
          timestamp: performance.now() * 1000,
          data: nalsToLengthPrefixed(sliceNals),
        });
        try { videoDecoder.decode(chunk); } catch (e) { droppedFrames++; console.warn('h264 decode failed:', e); }
      }
    }
  } else {
    // JPEG fallback (openh264 decode → JPEG encode in Rust)
    const bytes = Uint8Array.from(atob(data), c => c.charCodeAt(0));
    const blob = new Blob([bytes], { type: 'image/jpeg' });
    const url = URL.createObjectURL(blob);
    const img = new Image();
    img.onload = () => {
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
      URL.revokeObjectURL(url);
    };
    img.src = url;
  }

  updateStreamStats();
});

listen('frame-stats', (event) => {
  const stats = document.getElementById('stats');
  const kf = event.payload.keyframe ? ' [kf]' : '';
  stats.textContent = `${event.payload.fps.toFixed(1)} fps · ${event.payload.count}${kf}`;
  stats.classList.add('visible');
});

listen('frame-error', () => {
  if (connected) disconnect();
});

// ─── Input ────────────────────────────────────────────────────────────────────
function toggleControl() {
  if (!connected) return;
  controlMode = !controlMode;
  updateControlUI();
}

function updateControlUI() {
  const el = document.getElementById('control-toggle');
  el.textContent = controlMode ? 'controlling' : 'take control';
  el.classList.toggle('active', controlMode);
}

function scaleCoords(clientX, clientY) {
  const rect = canvas.getBoundingClientRect();
  const scaleX = frameWidth / rect.width;
  const scaleY = frameHeight / rect.height;
  return {
    x: Math.round((clientX - rect.left) * scaleX),
    y: Math.round((clientY - rect.top) * scaleY),
  };
}

function mapKey(e) {
  const key = e.key;
  const map = {
    'ArrowUp': 'Up', 'ArrowDown': 'Down', 'ArrowLeft': 'Left', 'ArrowRight': 'Right',
    ' ': 'Space', 'Delete': 'Delete', 'Backspace': 'Backspace',
    'Enter': 'Enter', 'Tab': 'Tab', 'Escape': 'Escape',
    'Shift': 'Shift', 'Control': 'Control', 'Alt': 'Alt', 'Meta': 'Meta',
    'Home': 'Home', 'End': 'End', 'PageUp': 'PageUp', 'PageDown': 'PageDown',
  };
  return map[key] || (key.length === 1 ? key : null);
}

async function sendInput(event) {
  try {
    await invoke('iroh_client_send_input', { event });
  } catch (e) {
    console.warn('input send failed:', e);
  }
}

canvas.addEventListener('mousemove', (e) => {
  if (!controlMode) return;
  const { x, y } = scaleCoords(e.clientX, e.clientY);
  sendInput({ t: 'mm', x, y });
});

canvas.addEventListener('mousedown', (e) => {
  if (!controlMode) return;
  e.preventDefault();
  const btn = e.button === 2 ? 2 : (e.button === 1 ? 3 : 1);
  sendInput({ t: 'md', b: btn });
});

canvas.addEventListener('mouseup', (e) => {
  if (!controlMode) return;
  e.preventDefault();
  const btn = e.button === 2 ? 2 : (e.button === 1 ? 3 : 1);
  sendInput({ t: 'mu', b: btn });
});

canvas.addEventListener('contextmenu', (e) => {
  if (!controlMode) return;
  e.preventDefault();
});

canvas.addEventListener('wheel', (e) => {
  if (!controlMode) return;
  e.preventDefault();
  sendInput({ t: 'ms', dx: Math.round(e.deltaX), dy: Math.round(e.deltaY) });
}, { passive: false });

window.addEventListener('keydown', (e) => {
  if (!controlMode) return;
  e.preventDefault();
  const k = mapKey(e);
  if (k) sendInput({ t: 'kd', k });
});

window.addEventListener('keyup', (e) => {
  if (!controlMode) return;
  e.preventDefault();
  const k = mapKey(e);
  if (k) sendInput({ t: 'ku', k });
});

window.addEventListener('keypress', (e) => {
  if (!controlMode) return;
  if (e.key.length === 1) {
    e.preventDefault();
    sendInput({ t: 'tx', s: e.key });
  }
});

// ─── PIN keyboard shortcuts ───────────────────────────────────────────────────
document.addEventListener('keydown', (e) => {
  const intro = document.getElementById('intro');
  if (intro.classList.contains('hidden')) return;
  const active = document.activeElement;
  if (!active || !active.classList.contains('overlay-input')) return;
  const pin = active.value.trim();
  if (!pin) return;
  const key = e.key.toLowerCase();
  if (key === 'c') {
    e.preventDefault();
    if (active.id === 'intro-pin-unreg') introConnectUnreg();
    else introConnectReg();
  } else if (key === 'r' && active.id === 'intro-pin-unreg') {
    e.preventDefault();
    introRegister();
  } else if (key === 'h' && active.id === 'intro-pin-reg') {
    e.preventDefault();
    introHost();
  }
});

// ─── PIN shortcut hints ───────────────────────────────────────────────────────
function updateIntroDes() {
  const unregVisible = document.getElementById('intro-unregistered').style.display !== 'none';
  const pinEl = unregVisible
    ? document.getElementById('intro-pin-unreg')
    : document.getElementById('intro-pin-reg');
  const desc = document.getElementById('intro-desc');
  if (!pinEl || !pinEl.value.trim()) {
    desc.textContent = 'type pin';
    return;
  }
  desc.textContent = unregVisible ? '[c]=connect · [r]=register' : '[c]=connect · [h]=host';
}
document.getElementById('intro-pin-unreg').addEventListener('input', updateIntroDes);
document.getElementById('intro-pin-reg').addEventListener('input', updateIntroDes);

listen('fido-done', () => {
  document.getElementById('tap-overlay-title').textContent = 'connecting';
  document.getElementById('tap-overlay-desc').textContent = 'Key recognised. Waiting for host to respond.';
  document.getElementById('tap-status').textContent = 'please wait...';
});

// ─── Expose to HTML onclick handlers ─────────────────────────────────────────
Object.assign(window, {
  introRegister, introConnectUnreg, introConnectReg, introHost,
  configBack, configHost,
  mainAction, connectHost, disconnect,
  unregister, unregisterCancel, unregisterConfirm,
  scanFido, togglePanel, toggleControl, saveEncoderConfig,
});

// ─── Init ─────────────────────────────────────────────────────────────────────
checkRegistration();
scanFido();
