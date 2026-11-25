// Run on the Fly VM. Browser profiles are fresh per pair and are removed on exit.
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const port = Number(process.argv[2] ?? 9333);
const coldId = "aqz-KE-bpKQ";
const print = (value) => console.log(JSON.stringify(value));

function installFrameProbe() {
  if (window !== window.top || window.ntFrameReceipts) return;
  const receipts = window.ntFrameReceipts = [];
  const emit = (kind, fields = {}) => {
    const event = { kind, at: performance.timeOrigin + performance.now(), ...fields };
    receipts.push(event);
    // A cold click can immediately destroy its document.
    if (kind === "click") window.ntReceipt?.(JSON.stringify(event));
  };
  const urlId = () => new URL(location.href).searchParams.get("v");
  const player = () => document.querySelector("#movie_player");
  const activeAd = (node) => Boolean(node && (
    node.classList.contains("ad-showing") ||
    [...node.querySelectorAll(".ytp-ad-preview-container, .ytp-ad-skip-button-container, .ytp-ad-player-overlay")]
      .some(element => element.checkVisibility({ visibilityProperty: true }) &&
        element.getBoundingClientRect().height > 0)
  ));

  let watched;
  const attached = new WeakSet();
  let trial;
  window.ntArm = (id, warm) => {
    const old = warm ? player()?.querySelector("video") : null;
    trial = { id, old, oldSrc: old?.currentSrc, oldTime: old?.currentTime, reset: false, ad: false };
  };
  if (urlId()) window.ntArm(urlId(), false);
  const sampleAd = () => {
    if (!trial || urlId() !== trial.id) return false;
    const ad = activeAd(player());
    if (ad !== trial.ad) emit("ad-state", { id: trial.id, ad });
    trial.ad = ad;
    return ad;
  };

  function frame(video, now, metadata) {
    video.requestVideoFrameCallback((nextNow, nextMetadata) => frame(video, nextNow, nextMetadata));
    if (!trial || urlId() !== trial.id || player()?.querySelector("video") !== video) return;
    const ad = sampleAd();
    // YouTube can reuse the old video element and MediaSource on a client-side transition.
    const changed = video !== trial.old || video.currentSrc !== trial.oldSrc ||
      trial.reset || metadata.mediaTime + 0.3 < trial.oldTime;
    if (!changed) return;
    let playerId;
    try { playerId = player().getVideoData().video_id; } catch {}
    const kind = ad ? "ad-frame" : playerId === trial.id ? "content-frame" : "unclassified-frame";
    trial.frames ??= new Set();
    if (trial.frames.has(kind)) return;
    trial.frames.add(kind);
    emit(kind, {
      id: trial.id, playerId, mediaTime: metadata.mediaTime,
      presentedAt: performance.timeOrigin + now,
      width: metadata.width, height: metadata.height,
    });
  }

  function observe() {
    sampleAd();
    const video = player()?.querySelector("video");
    if (!video || video === watched) return;
    watched = video;
    if (attached.has(video)) return;
    attached.add(video);
    for (const name of ["loadstart", "emptied"]) {
      video.addEventListener(name, () => {
        if (trial && video === trial.old) trial.reset = true;
      });
    }
    video.requestVideoFrameCallback((now, metadata) => frame(video, now, metadata));
  }

  addEventListener("click", (event) => {
    if (!event.isTrusted) return;
    const href = event.target.closest("a[href]")?.href;
    const id = href && new URL(href).searchParams.get("v");
    if (id) emit("click", { id });
  }, true);
  new MutationObserver(observe).observe(document, {
    subtree: true, childList: true, attributes: true, attributeFilter: ["class"],
  });
  setInterval(observe, 25);
}

async function connect() {
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const target = targets.find(({ type }) => type === "page");
      if (target) {
        const socket = new WebSocket(target.webSocketDebuggerUrl);
        await once(socket, "open");
        return socket;
      }
    } catch {}
    await sleep(100);
  }
  throw Error("Chromium CDP unavailable");
}

async function runPair(repeat) {
  const profile = await mkdtemp(join(tmpdir(), "nt-web-profile-"));
  const browser = spawn("chromium", [
    `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check",
    "--ozone-platform=wayland", "--window-size=1280,800", "--start-fullscreen",
    "--mute-audio", "--autoplay-policy=no-user-gesture-required", "--disable-sync",
    "--disable-extensions", "--disable-component-extensions-with-background-pages",
    `--remote-debugging-port=${port}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "inherit"], detached: true });
  let socket;
  try {
    socket = await connect();
    let sequence = 0;
    const pending = new Map();
    const events = [];
    socket.addEventListener("message", ({ data }) => {
      const message = JSON.parse(data);
      if (message.method === "Runtime.bindingCalled" && message.params.name === "ntReceipt") {
        const event = JSON.parse(message.params.payload);
        events.push(event);
        print({ repeat, ...event });
      }
      pending.get(message.id)?.(message);
    });
    async function send(method, params = {}) {
      const id = ++sequence;
      const { promise, resolve } = Promise.withResolvers();
      pending.set(id, resolve);
      const timer = setTimeout(() => resolve({ error: { message: "deadline" } }), 5000);
      try {
        socket.send(JSON.stringify({ id, method, params }));
        const result = await promise;
        if (result.error || result.result?.exceptionDetails) throw Error(`${method}: ${JSON.stringify(result)}`);
        return result.result;
      } finally {
        clearTimeout(timer);
        pending.delete(id);
      }
    }
    const evaluate = async (expression) => (await send("Runtime.evaluate", {
      expression, returnByValue: true,
    })).result.value;

    await send("Page.enable");
    await send("Runtime.enable");
    await send("Runtime.addBinding", { name: "ntReceipt" });
    const source = `(${installFrameProbe})()`;
    await send("Page.addScriptToEvaluateOnNewDocument", { source });
    await evaluate(source);
    await evaluate(`document.body.innerHTML = '<a href="https://www.youtube.com/watch?v=${coldId}">Open clip</a>'`);

    async function measure(mode, anchor, id, deadline) {
      const from = events.length;
      let mouseupAt;
      for (const type of ["mousePressed", "mouseReleased"]) {
        if (type === "mouseReleased") mouseupAt = Date.now();
        await send("Input.dispatchMouseEvent", { x: anchor.x, y: anchor.y, type, button: "left", clickCount: 1 });
      }
      const end = Date.now() + deadline;
      let status = "deadline";
      const seen = new Set();
      while (Date.now() < end) {
        try {
          const state = await evaluate(`(() => {
            const text = document.body?.innerText ?? "";
            let blocked;
            if (/sign in to confirm you.re not a bot/i.test(text)) blocked = "bot check";
            if (/video unavailable|this video is not available/i.test(text)) blocked = "video unavailable";
            if (location.hostname.includes("consent.")) blocked = "consent interstitial";
            return { blocked, receipts: window.ntFrameReceipts ?? [] };
          })()`);
          for (const event of state.receipts) {
            const key = JSON.stringify(event);
            if (event.at < mouseupAt || event.kind === "click" || seen.has(key)) continue;
            seen.add(key);
            events.push(event);
            print({ repeat, ...event });
          }
          if (events.slice(from).some(event => event.kind === "content-frame" && event.id === id)) {
            status = "complete";
            break;
          }
          if (state.blocked) {
            status = state.blocked;
            break;
          }
        } catch {} // The old document can disappear during a navigation.
        await sleep(100);
      }
      const receipt = events.slice(from).filter(event => event.id === id);
      const click = receipt.find(event => event.kind === "click") ?? { at: mouseupAt };
      const first = receipt.find(event => event.presentedAt && click && event.presentedAt >= click.at);
      const content = receipt.find(event => event.kind === "content-frame" && click && event.presentedAt >= click.at);
      let adStart;
      let adMs = 0;
      for (const event of receipt) {
        if (!content || event.at > content.at || event.kind !== "ad-state") continue;
        if (event.ad) adStart = event.at;
        else if (adStart !== undefined) {
          adMs += event.at - adStart;
          adStart = undefined;
        }
      }
      const classified = content && adStart === undefined &&
        !receipt.some(event => event.kind === "unclassified-frame" && event.at <= content.at);
      const contentMs = content ? content.presentedAt - click.at : null;
      print({
        kind: "result", repeat, mode, id, status,
        click_clock: click.kind === "click" ? "trusted page click" : "CDP mouse release send",
        any_frame_ms: first ? first.presentedAt - click.at : null,
        actual_frame_elapsed_ms: classified ? contentMs : null,
        actual_frame_excluding_ad_ms: classified ? Math.max(0, contentMs - adMs) : null,
        ad_mode_ms: classified ? adMs : null,
      });
      return Boolean(content);
    }

    const cold = await evaluate(`(() => {
      const r = document.querySelector("a").getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
    })()`);
    if (!await measure("cold", cold, coldId, 60000)) {
      print({ kind: "result", repeat, mode: "warm", status: "cold playback unavailable" });
      return;
    }
    await sleep(3000);
    const warm = await evaluate(`(() => {
      const links = [...document.querySelectorAll('#related a[href^="/watch?"]')];
      const link = links.find(link => link.getBoundingClientRect().height >= 20 &&
        new URL(link.href).searchParams.get("v") !== "${coldId}");
      if (!link) return null;
      link.scrollIntoView({ block: "center" });
      const r = link.getBoundingClientRect();
      const id = new URL(link.href).searchParams.get("v");
      window.ntArm(id, true);
      return { id, x: r.x + r.width / 2, y: r.y + r.height / 2 };
    })()`);
    if (!warm) print({ kind: "result", repeat, mode: "warm", status: "no visible related link" });
    else await measure("warm", warm, warm.id, 130000);
  } finally {
    socket?.close();
    if (browser.pid && browser.exitCode === null && browser.signalCode === null) {
      const exited = once(browser, "exit");
      try { process.kill(-browser.pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; }
      await exited;
    }
    await rm(profile, { recursive: true, force: true });
  }
}

for (let repeat = 1; repeat <= 3; repeat++) {
  try {
    await runPair(repeat);
  } catch (error) {
    print({ kind: "error", repeat, error: error.message });
  }
}
