// Run one DevTools evaluation on the VM Chromium tab.
const [port, expression, action] = process.argv.slice(2);
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = targets.find(target => target.type === "page");
if (!page) throw new Error("Chromium tab unavailable");

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});

let nextId = 1;
function send(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    ws.onmessage = ({ data }) => {
      const message = JSON.parse(data);
      if (message.id !== id) return;
      if (message.error || message.result.exceptionDetails) reject(Error(`CDP ${method} failed`));
      else resolve(message.result);
    };
    ws.send(JSON.stringify({ id, method, params }));
  });
}

try {
  const evaluation = await send("Runtime.evaluate", { expression, returnByValue: true });
  const value = evaluation.result.value ?? null;
  // The recording can click a visible Skip button just as a viewer would.
  if (action === "click" && Number.isFinite(value?.x) && Number.isFinite(value?.y)) {
    const point = { x: value.x, y: value.y };
    await send("Input.dispatchMouseEvent", { ...point, type: "mouseMoved" });
    for (const type of ["mousePressed", "mouseReleased"]) {
      await send("Input.dispatchMouseEvent", { ...point, type, button: "left", clickCount: 1 });
    }
  }
  console.log(JSON.stringify(value));
} finally {
  ws.close();
}
