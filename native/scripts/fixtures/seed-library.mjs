// Seeds a config dir with a translated Spanish book, through the headless
// Tauri app's E2E bridge and the LLM sim, for the native smoke and UI tests.
// Needs: cargo build -p app -p e2e-sims --features app/e2e-bridge
// Usage: node seed-library.mjs <config-dir> [paragraphs=40] [book.epub]
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
const ROOT = new URL("../../../", import.meta.url).pathname;
const require = createRequire(ROOT + "site/package.json");
const WebSocket = require("ws");

const [,, dir, paragraphsArg, epubPath] = process.argv;
const bin = ROOT + "target/debug";
const clean = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith("FLTS_")));

function line(child, pred) {
  return new Promise((res, rej) => {
    let buf = "";
    child.stdout.on("data", (c) => {
      buf += c;
      for (const l of buf.split("\n")) if (pred(l)) return res(l);
    });
    child.on("exit", (code) => rej(new Error("exited " + code)));
  });
}

// Children must not outlive a failed run.
const children = [];
process.on("exit", () => children.forEach((c) => c.kill()));
process.on("uncaughtException", (e) => { console.error(e); process.exit(1); });
process.on("unhandledRejection", (e) => { console.error(e); process.exit(1); });
const sims = spawn(path.join(bin, "flts-e2e-sims"), { stdio: ["pipe", "pipe", "inherit"], env: clean });
children.push(sims);
const ports = JSON.parse(await line(sims, (l) => l.trim().startsWith("{")));
fs.mkdirSync(dir, { recursive: true });
fs.writeFileSync(path.join(dir, "config.json"), JSON.stringify({
  targetLanguageId: "eng", translationProvider: "google", geminiApiKey: "sim-key",
  model: "models/gemini-2.5-flash", ankiEndpoint: `http://127.0.0.1:${ports.anki}`, syncEnabled: false,
}, null, 2));
fs.writeFileSync(path.join(dir, "sims.json"), JSON.stringify(ports));

const env = { ...clean, FLTS_E2E_BRIDGE_PORT: "0", FLTS_CONFIG_DIR: dir,
  FLTS_GEMINI_BASE_URL: `http://127.0.0.1:${ports.llm}/v1beta/`, FLTS_DISABLE_SYNC: "1", FLTS_DISABLE_KEYRING: "1" };
const app = spawn(path.join(bin, "app"), { stdio: ["ignore", "pipe", "inherit"], env });
children.push(app);
const l = await line(app, (l) => l.startsWith("FLTS_E2E_BRIDGE_LISTENING"));
const port = JSON.parse(l.slice("FLTS_E2E_BRIDGE_LISTENING".length)).port;
const ws = new WebSocket(`ws://127.0.0.1:${port}/bridge`);
await new Promise((r) => ws.on("open", r));
let id = 0; const waiting = new Map();
ws.on("message", (m) => { const f = JSON.parse(m); if (f.event === "paragraph_translation_finished" && f.payload.error) console.log("FINISHED-ERR", f.payload.error.slice(0, 300)); if (f.id != null && waiting.has(f.id)) { waiting.get(f.id)(f); waiting.delete(f.id); } });
const call = (cmd, args = {}) => new Promise((res, rej) => { const i = ++id; waiting.set(i, (f) => "ok" in f ? res(f.ok) : rej(new Error(JSON.stringify(f.err)))); ws.send(JSON.stringify({ id: i, cmd, args })); });

const words = "el gato negro duerme sobre la mesa mientras la lluvia cae despacio en la ciudad antigua y nadie sabe cuándo volverá el sol a las calles vacías".split(" ");
const n = Number(paragraphsArg ?? 40);
const paras = [];
for (let p = 0; p < n; p++) {
  const len = 30 + ((p * 37) % 90);
  const s = [];
  for (let i = 0; i < len; i++) s.push(words[(p * 7 + i * 3) % words.length]);
  paras.push(s.join(" ").replace(/^./, (c) => c.toUpperCase()) + ".");
}
const dict = { el: "the", gato: "cat", negro: "black", duerme: "sleeps", sobre: "on", la: "the", mesa: "table",
  mientras: "while", lluvia: "rain", cae: "falls", despacio: "slowly", en: "in", ciudad: "city", antigua: "old",
  y: "and", nadie: "nobody", sabe: "knows", "cuándo": "when", "volverá": "will return", sol: "sun", a: "to",
  las: "the", calles: "streets", "vacías": "empty" };
const scripts = paras.map((text) => {
  const tokens = text.slice(0, -1).split(" ");
  const wl = tokens.map((w) => {
    const key = w.toLowerCase();
    const t = dict[key] ?? key;
    return { o: w, t: [t], n: null, p: false, g: { lf: key, lt: t, pos: "common_noun", pl: null, pe: null, te: null, ca: null, ot: null } };
  });
  wl.push({ o: ".", t: ["."], n: null, p: true, g: { lf: ".", lt: ".", pos: "punctuation", pl: null, pe: null, te: null, ca: null, ot: null } });
  return { matchSubstring: text, translation: { s: [{ ft: tokens.map((w) => dict[w.toLowerCase()] ?? w).join(" ") + ".", wl }] } };
});
await fetch(`http://127.0.0.1:${ports.llm}/_sim/seed`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ scripts }) });
for (let i = 0; i < 60; i++) { try { await call("list_books"); break; } catch { await new Promise(r => setTimeout(r, 250)); } }
const bookId = await call("import_plain_text", { title: "Lluvia en la ciudad", text: paras.join("\n\n"), sourceLanguageId: "spa" });
const ids = await call("get_book_chapter_paragraph_ids", { bookId, chapterId: 0 });
// Translate every other paragraph so both kinds show.
for (const pid of ids.filter((_, i) => i % 2 === 0 || i < 6)) await call("translate_paragraph", { bookId, paragraphId: pid, model: "models/gemini-2.5-flash", useCache: true });
for (let i = 0; i < 20; i++) {
  const t = await call("get_paragraph_texts_batch", { bookId, paragraphIds: [ids[0], ids[ids.length - 2]] });
  if (t.paragraphs.every((p) => p.translated)) break;
  await new Promise(r => setTimeout(r, 500));
}
if (epubPath) {
  const book = await call("parse_epub", { epubBase64: fs.readFileSync(epubPath).toString("base64") });
  await call("import_epub", { book, sourceLanguageId: "spa" });
}
const html = await call("get_paragraph_texts_batch", { bookId, paragraphIds: ids.slice(0, 2) });
console.log(JSON.stringify({ bookId, paragraphs: ids.length, sample: html.paragraphs[0] }).slice(0, 600));
ws.close(); app.kill("SIGTERM");
await new Promise(r => app.on("exit", r));
sims.stdin.end(); sims.kill();
