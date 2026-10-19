// Show the keys pressed in a recording: keycap badges laid over a VHS
// recording at the moments the keys act, as <out>.mp4.
//
//   node keys.mjs <tape> <recording.mp4|.gif> --out <file.mp4>
//     [--position bottom|top] [--hold 0.9] [--gif [--gif-width <px>]]
//
// `--gif` writes <out>.gif beside the MP4, at the recording's width unless
// `--gif-width` says otherwise. run.sh calls this for every tape with
// `# hint:` lines and keeps the GIF with the badges; the release posts call
// it for their own tapes, whose shared steps (`Source tapes/_….tape`) are
// read from this directory. Needs Node 20.10+, ffmpeg and Chrome or
// Chromium (CHROME_PATH to name one).
//
// Timing comes in two passes. The tape gives the order of the keys and when
// each falls by its own clock (Sleep, the typing speed, Hide/Show — the
// shared tapes it sources are read too); VHS drifts from that clock by a
// tenth of a second or two, so each key is then moved onto the first change
// of the screen near its moment, and the drift found carries on to the next.
// A key that changes nothing on screen keeps its computed moment.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SHARED_TAPES = dirname(fileURLToPath(import.meta.url));

// The DevTools port headless Chrome picked, read from its stderr.
function chromePort(chrome, profile) {
  return new Promise((done, fail) => {
    let text = "";
    const timer = setTimeout(() => stop(new Error("Chrome did not start")), 20000);
    const data = (chunk) => {
      text += chunk;
      const match = text.match(/DevTools listening on ws:\/\/[^:]+:(\d+)\//);
      if (match) {
        clean();
        done(Number(match[1]));
      }
    };
    const exited = (code, signal) => stop(new Error(`Chrome exited before startup: ${code ?? signal}`));
    const clean = () => {
      clearTimeout(timer);
      chrome.stderr.off("data", data);
      chrome.off("error", stop);
      chrome.off("exit", exited);
    };
    function stop(cause) {
      clean();
      chrome.kill("SIGKILL");
      rmSync(profile, { recursive: true, force: true });
      fail(cause);
    }
    chrome.stderr.on("data", data);
    chrome.on("error", stop);
    chrome.on("exit", exited);
  });
}

const argv = process.argv.slice(2);
const flag = (name) => (argv.includes(name) ? argv[argv.indexOf(name) + 1] : undefined);
const [tapeArg, videoArg] = argv.filter((arg, i) => !arg.startsWith("--") && !["--out", "--position", "--hold", "--gif-width"].includes(argv[i - 1]));
const out = flag("--out");
if (!tapeArg || !videoArg || !out) {
  console.error("usage: node keys.mjs <tape> <recording.mp4|.gif> --out <file.mp4> [--position bottom|top] [--hold 0.9] [--gif [--gif-width <px>]]");
  process.exit(2);
}
const HOLD = Number(flag("--hold") ?? 0.9);
const LEAD = 0.08; // the badge shows this long before the screen changes

// ---- 1. The tape's clock -------------------------------------------------

const KEYS = new Set(["Enter", "Backspace", "Delete", "Insert", "Tab", "Space", "Escape", "Up", "Down", "Left", "Right", "PageUp", "PageDown", "Home", "End"]);
const NAMES = { Up: "↑", Down: "↓", Left: "←", Right: "→", Escape: "Esc", Backspace: "⌫", Enter: "Enter" };

function tokens(line) {
  const out = [];
  for (let i = 0; i < line.length; ) {
    if (/\s/.test(line[i])) i++;
    else if (`"'\``.includes(line[i])) {
      const quote = line[i];
      const end = line.indexOf(quote, i + 1);
      out.push({ text: line.slice(i + 1, end < 0 ? undefined : end) });
      i = end < 0 ? line.length : end + 1;
    } else {
      let j = i;
      while (j < line.length && !/\s/.test(line[j])) j++;
      out.push(line.slice(i, j));
      i = j;
    }
  }
  return out;
}

const seconds = (text) => {
  const m = String(text).match(/^([\d.]+)(ms|s|m)?$/);
  if (!m) return 0;
  return Number(m[1]) * (m[2] === "ms" ? 0.001 : m[2] === "m" ? 60 : 1);
};

// A `# hint: <parts> [for=<time>] [color=<css>] [bg=<css>]` comment shows a
// badge of its own at that point of the tape: each part a key or chord
// (`Ctrl+F8`) or a quoted text (`"continue"`); `for` how long it stays (else
// HOLD after it), `color` its keys' and text's colour, `bg` its background.
// A badge still gives way to the next one. A tape with any hint shows its
// hints only, not the keys it sends, so a stand-in chord bound for VHS can be
// shown as the real key, or a click as words.
const HINT_OPTIONS = /^(for|color|bg)=(.+)$/;
function parseHint(spec) {
  const hint = { parts: [], options: {} };
  for (const word of tokens(spec)) {
    const option = typeof word === "string" && word.match(HINT_OPTIONS);
    if (option) hint.options[option[1]] = option[1] === "for" ? seconds(option[2]) : option[2];
    else if (typeof word !== "string") hint.parts.push({ text: word.text });
    else if (word !== "·") hint.parts.push({ label: word.split("+").map((part) => NAMES[part] ?? part), count: 1 });
  }
  return hint;
}

const state = { time: 0, visible: true, speed: 0.05, events: [], uncertain: false, explicit: false };
function play(file) {
  for (const raw of readFileSync(file, "utf8").split("\n")) {
    const line = raw.trim();
    const hint = line.match(/^#\s*hint:\s*(.+)$/);
    if (hint) {
      state.explicit = true;
      if (state.visible) state.events.push({ t: state.time, hint: parseHint(hint[1]) });
      continue;
    }
    if (!line || line.startsWith("#")) continue;
    const list = tokens(line);
    for (let i = 0; i < list.length; i++) {
      const word = list[i];
      if (typeof word !== "string") continue;
      const [name, at] = word.split("@");
      const speed = at ? seconds(at) : state.speed;
      const pass = (t) => { if (state.visible) state.time += t; };
      if (name === "Set") {
        if (list[i + 1] === "TypingSpeed") state.speed = seconds(list[i + 2]);
        break;
      } else if (name === "Source") {
        const path = list[i + 1];
        play(existsSync(resolve(dirname(file), path)) ? resolve(dirname(file), path) : join(SHARED_TAPES, path));
        break;
      } else if (["Output", "Screenshot", "Require", "Env"].includes(name)) break;
      else if (name === "Hide") state.visible = false;
      else if (name === "Show") state.visible = true;
      else if (name === "Sleep") pass(seconds(list[++i]));
      else if (name.startsWith("Wait")) state.uncertain = true;
      else if (name === "Type") {
        const text = list[++i]?.text ?? "";
        if (state.visible && text) state.events.push({ t: state.time, label: text.length === 1 ? [text] : null, text: text.length > 1 ? text : null });
        pass(text.length * speed);
      } else if (KEYS.has(name) || /^(Ctrl|Alt|Shift)\+/.test(name)) {
        const count = /^\d+$/.test(list[i + 1] ?? "") ? Number(list[++i]) : 1;
        if (state.visible) {
          const parts = name.split("+").map((part) => NAMES[part] ?? part);
          state.events.push({ t: state.time, label: parts, count });
        }
        pass(count * speed);
      }
    }
  }
}
play(resolve(tapeArg));
if (state.explicit) state.events = state.events.filter((event) => event.hint);

// ---- 2. Snap each key to the screen ---------------------------------------

const video = resolve(videoArg);
// A GIF beside the MP4 carries no compression noise: measure change on it.
const gifBeside = video.replace(/\.mp4$/i, ".gif");
const measured = gifBeside !== video && existsSync(gifBeside) ? gifBeside : video;
const [vw, vh, rate] = execFileSync("ffprobe", ["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate", "-of", "csv=p=0", video], { encoding: "utf8" }).trim().split(",");
const fps = rate.includes("/") ? Number(rate.split("/")[0]) / Number(rate.split("/")[1]) : Number(rate);
const SW = 320;
const SH = 188;
// The menu bar (top 5%) ticks every second with the network counters: leave it out.
const raw = execFileSync("ffmpeg", ["-loglevel", "error", "-i", measured, "-vf", `crop=iw:ih*0.95:0:ih*0.05,scale=${SW}:${SH},format=gray`, "-f", "rawvideo", "-"], { maxBuffer: 1 << 30 });
const frames = raw.length / (SW * SH);
const change = new Float64Array(frames);
for (let f = 1; f < frames; f++) {
  // How many pixels changed clearly: one typed character is enough.
  let count = 0;
  for (let p = 0; p < SW * SH; p++) if (Math.abs(raw[f * SW * SH + p] - raw[(f - 1) * SW * SH + p]) > 24) count++;
  change[f] = count;
}
const THRESHOLD = 2;
let drift = 0;
for (const event of state.events) {
  const guess = event.t + drift;
  const from = Math.max(1, Math.floor((guess - 0.35) * fps));
  const to = Math.min(frames - 1, Math.ceil((guess + 0.35) * fps));
  let hit = -1;
  for (let f = from; f <= to; f++) if (change[f] > THRESHOLD) { hit = f; break; }
  if (hit >= 0) {
    const seen = hit / fps;
    drift = seen - event.t;
    event.at = Math.max(0, seen - LEAD);
    event.snapped = true;
  } else event.at = Math.max(0, guess - LEAD);
}

// Fold presses of one key in a row into one part (Down ×3), and keys that
// follow each other faster than a reader takes them in into one badge, a chain.
// A hint is a badge as written; the folding is for keys read off the tape.
const parts = [];
for (const event of state.explicit ? [] : state.events) {
  const last = parts[parts.length - 1];
  if (last && !last.text && !event.text && last.label.join("+") === event.label.join("+") && event.at - last.at < 0.6) last.count += event.count;
  else parts.push({ ...event });
}
const shown = state.explicit
  ? state.events.map((event) => ({ at: event.at, options: event.hint.options, parts: event.hint.parts.map((part) => ({ ...part, at: event.at, snapped: event.snapped })) }))
  : [];
for (const part of parts) {
  const badge = shown[shown.length - 1];
  if (badge && part.at - badge.parts[badge.parts.length - 1].at < 0.45) badge.parts.push(part);
  else shown.push({ at: part.at, parts: [part] });
}
shown.forEach((badge, i) => {
  badge.end = badge.options?.for ? badge.at + badge.options.for : Math.max(badge.parts[badge.parts.length - 1].at + HOLD, badge.at + 0.6);
  if (shown[i + 1]) badge.end = Math.min(badge.end, shown[i + 1].at);
});

// ---- 3. Draw the badges ----------------------------------------------------

const escape = (text = "") => String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const scale = Number(vh) / 960;
const partHtml = (part) =>
  part.text
    ? `<span class="typed">${escape(part.text)}</span>`
    : part.label.map((key) => `<kbd>${escape(key)}</kbd>`).join('<span class="plus">+</span>') + (part.count > 1 ? `<span class="count">×${part.count}</span>` : "");
// Colours go in as CSS custom properties; a value is CSS, quotes and angle
// brackets left out.
const css = (value) => String(value).replace(/["<>;]/g, "");
const badgeStyle = ({ color, bg } = {}) =>
  [color && `--c:${css(color)}`, bg && `--bg:${css(bg)}`].filter(Boolean).join(";");
const badgeHtml = (badge, i) => `<div class="badge" id="b${i}" style="${badgeStyle(badge.options)}">${badge.parts.map(partHtml).join('<span class="then">·</span>')}</div>`;
const page = `<!doctype html><html><head><meta charset="utf-8"><style>
  html,body{margin:0;background:transparent}
  body{display:flex;flex-direction:column;align-items:flex-start;gap:20px;padding:20px;font-family:ui-monospace,"SF Mono",Menlo,Consolas,monospace}
  .badge{display:inline-flex;align-items:center;gap:${10 * scale}px;padding:${12 * scale}px ${16 * scale}px;border-radius:${14 * scale}px;
    background:var(--bg,rgba(12,12,12,.88));border:1px solid rgba(255,255,255,.18);box-shadow:0 ${6 * scale}px ${24 * scale}px rgba(0,0,0,.45)}
  kbd{font:inherit;color:var(--c,#f2f2f2);font-size:${30 * scale}px;line-height:1;padding:${10 * scale}px ${15 * scale}px ${8 * scale}px;
    border:1px solid rgba(255,255,255,.3);border-bottom-width:${4 * scale}px;border-radius:${9 * scale}px;background:#262626}
  .plus,.count,.then{color:#9a9a9a;font-size:${26 * scale}px}
  .then{margin:0 ${6 * scale}px}
  .typed{color:var(--c,#f2f2f2);font-size:${30 * scale}px;padding:${8 * scale}px ${6 * scale}px;border-bottom:2px solid var(--c,#6be0a0)}
</style></head><body>${shown.map(badgeHtml).join("")}</body></html>`;

const work = mkdtempSync(join(tmpdir(), "feature-keys-"));
const chromeBinary = [process.env.CHROME_PATH, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/chromium", "/usr/bin/google-chrome"].find((p) => p && existsSync(p));
const profile = join(work, "profile");
mkdirSync(profile);
writeFileSync(join(work, "badges.html"), page);
const chrome = spawn(chromeBinary, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", "about:blank"]);
const done = async () => {
  const exited = new Promise((r) => (chrome.exitCode !== null || chrome.signalCode !== null ? r() : chrome.once("exit", r)));
  chrome.kill("SIGKILL");
  await exited;
};
try {
  const port = await chromePort(chrome, profile);
  const tab = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT" })).json();
  const socket = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  let loaded = () => {};
  socket.onmessage = (m) => {
    const data = JSON.parse(m.data);
    if (data.method === "Page.loadEventFired") loaded();
    if (data.id && pending.has(data.id)) { pending.get(data.id)(data.result); pending.delete(data.id); }
  };
  await new Promise((r) => (socket.onopen = r));
  const send = (method, params = {}) => new Promise((r) => { const call = ++id; pending.set(call, r); socket.send(JSON.stringify({ id: call, method, params })); });
  await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 2400, deviceScaleFactor: 1, mobile: false });
  await send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
  const load = new Promise((r) => { loaded = r; setTimeout(r, 5000); });
  await send("Page.navigate", { url: pathToFileURL(join(work, "badges.html")).href });
  await load;
  for (let i = 0; i < shown.length; i++) {
    const box = (await send("Runtime.evaluate", { expression: `(() => { const r = document.getElementById("b${i}").getBoundingClientRect(); return { x: r.x - 16, y: r.y - 12, width: r.width + 32, height: r.height + 32 }; })()`, returnByValue: true })).result.value;
    const shot = await send("Page.captureScreenshot", { format: "png", clip: { ...box, scale: 1 } });
    writeFileSync(join(work, `b${i}.png`), Buffer.from(shot.data, "base64"));
  }
  socket.close();
} finally {
  await done();
}

// ---- 4. Lay them over the recording ---------------------------------------

const duration = Number(execFileSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", video], { encoding: "utf8" }).trim());
const top = flag("--position") === "top";
const inputs = ["-i", video];
const chain = [];
let last = "0:v";
shown.forEach((badge, i) => {
  inputs.push("-loop", "1", "-t", String(duration), "-i", join(work, `b${i}.png`));
  const fadeIn = Math.min(0.12, (badge.end - badge.at) / 3);
  const fadeOut = Math.min(0.18, (badge.end - badge.at) / 3);
  chain.push(`[${i + 1}:v]format=rgba,fade=t=in:st=${badge.at.toFixed(3)}:d=${fadeIn.toFixed(3)}:alpha=1,fade=t=out:st=${(badge.end - fadeOut).toFixed(3)}:d=${fadeOut.toFixed(3)}:alpha=1[k${i}]`);
  const y = top ? `${Math.round(70 * scale)}` : `H-h-${Math.round(56 * scale)}`;
  chain.push(`[${last}][k${i}]overlay=x=(W-w)/2:y=${y}:enable='between(t,${badge.at.toFixed(3)},${badge.end.toFixed(3)})'[v${i}]`);
  last = `v${i}`;
});
const target = resolve(out);
mkdirSync(dirname(target), { recursive: true });
execFileSync("ffmpeg", ["-loglevel", "error", "-y", ...inputs, "-filter_complex", chain.join(";") || "null", "-map", `[${last}]`, "-t", String(duration), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-movflags", "+faststart", target]);
if (argv.includes("--gif")) {
  const width = flag("--gif-width") ?? "iw";
  execFileSync("ffmpeg", ["-loglevel", "error", "-y", "-i", target, "-vf", `fps=20,scale=${width}:-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4`, target.replace(/\.mp4$/, ".gif")]);
}
rmSync(work, { recursive: true, force: true });
console.log(JSON.stringify({
  out: target,
  size: `${vw}x${vh}`,
  uncertain: state.uncertain,
  badges: shown.map((b) => ({
    keys: b.parts.map((p) => p.text ?? p.label.join("+") + (p.count > 1 ? ` x${p.count}` : "")).join(" · "),
    at: +b.at.toFixed(2),
    until: +b.end.toFixed(2),
    snapped: b.parts.map((p) => !!p.snapped),
  })),
}, null, 2));
