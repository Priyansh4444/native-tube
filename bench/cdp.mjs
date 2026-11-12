// Drives YouTube in a Chromium-based browser over the DevTools protocol for the
// playback benchmark. Reads commands on stdin, prints JSON events on stdout.
//
//   open <videoId> cold   full page load of /watch?v=<id> in the tab
//   open <videoId> warm   in-app (single-page) navigation, as clicking a video on youtube.com does
//   stats                 playback position, resolution, dropped frames
//   quit
//
// "first_frame" is reported when the <video> has advanced past 0 while showing the
// requested video and no ad. Timing uses the page's own clock (performance.now) from
// just before the navigation, polled every 20 ms.
//
// usage: node bench/cdp.mjs <devtools port>

import readline from "node:readline";

const port = process.argv[2] ?? "9333";
const emit = (o) => process.stdout.write(JSON.stringify(o) + "\n");

async function pageTarget() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) return page;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("no page target");
}

const target = await pageTarget();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
let nextId = 1;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
};
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  return r.result?.result?.value;
};

await send("Page.enable");
await send("Runtime.enable");

const PROBE = `(() => {
  const v = document.querySelector('#movie_player video') || document.querySelector('video');
  const p = document.querySelector('#movie_player');
  const id = new URLSearchParams(location.search).get('v');
  if (!v) return { now: performance.now(), origin: performance.timeOrigin, id, has: false };
  let pid = null;
  try { pid = p.getVideoData().video_id; } catch {}
  return { now: performance.now(), origin: performance.timeOrigin, id, pid, src: v.currentSrc, has: true, t: v.currentTime, ad: !!p && p.classList.contains('ad-showing'),
           w: v.videoWidth, h: v.videoHeight, paused: v.paused,
           dropped: v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality().droppedVideoFrames : null,
           total: v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality().totalVideoFrames : null };
})()`;

// `oldSrc` is the media source of the video that was playing before a warm navigation:
// the new video's first frame can only come after the element switched away from it.
async function waitFirstFrame(id, t0Page, deadlineMs, origin, oldSrc) {
  let firstAny = null;
  let reloaded = false;
  let sawAd = false;
  const end = Date.now() + deadlineMs;
  while (Date.now() < end) {
    const s = await evaluate(PROBE).catch(() => null);
    if (s && origin !== undefined && s.origin !== origin) reloaded = true;
    const stale = s && !reloaded && oldSrc !== undefined && s.src === oldSrc;
    if (s && s.has && s.t > 0.01 && !stale && (s.pid === null || s.pid === id)) {
      if (s.ad) sawAd = true;
      if (firstAny === null) firstAny = s.now - t0Page;
      if (!s.ad && s.id === id) return { ok: true, ms: s.now - t0Page, any_ms: firstAny, ad: sawAd, size: `${s.w}x${s.h}`, reloaded };
    }
    await new Promise((r) => setTimeout(r, 20));
  }
  return { ok: false, ms: null, any_ms: firstAny, ad: sawAd, reloaded };
}

const rl = readline.createInterface({ input: process.stdin });
emit({ event: "ready" });
for await (const line of rl) {
  const [cmd, id, mode] = line.trim().split(/\s+/);
  if (cmd === "open") {
    let t0Page;
    if (mode === "cold") {
      // Page clock restarts with the new document, so measure cold loads on the node clock.
      const t0 = Date.now();
      await send("Page.navigate", { url: `https://www.youtube.com/watch?v=${id}` });
      // performance.now() of the new page counts from its navigation start, which is ~t0.
      t0Page = 0;
      const r = await waitFirstFrame(id, t0Page, 60000);
      emit({ event: "first_frame", id, cold: true, ...r, wall_ms: Date.now() - t0 });
    } else {
      // YouTube's own single-page navigation: the same event its links fire.
      const origin = await evaluate(`performance.timeOrigin`);
      const oldSrc = (await evaluate(PROBE))?.src;
      const t0 = Date.now();
      t0Page = await evaluate(`(() => {
        const t = performance.now();
        const app = document.querySelector('ytd-app');
        app.dispatchEvent(new CustomEvent('yt-navigate', { bubbles: true, composed: true, detail: { endpoint: {
          commandMetadata: { webCommandMetadata: { url: '/watch?v=${id}', webPageType: 'WEB_PAGE_TYPE_WATCH', rootVe: 3832 } },
          watchEndpoint: { videoId: '${id}' } } } }));
        return t;
      })()`);
      const r = await waitFirstFrame(id, t0Page, 60000, origin, oldSrc);
      // If YouTube reloaded the page instead, the page clock restarted: fall back to the wall clock.
      if (r.reloaded && r.ok) r.ms = Date.now() - t0;
      emit({ event: "first_frame", id, cold: false, ...r, wall_ms: Date.now() - t0 });
    }
  } else if (cmd === "stats") {
    emit({ event: "stats", ...(await evaluate(PROBE)) });
  } else if (cmd === "quit") {
    break;
  }
}
ws.close();
process.exit(0);
