// Starts the e2e sims with one scripted translation per paragraph of a
// generated text, for the native translate smoke test. The sims exit when
// stdin closes, so keep it open: tail -f /dev/null | node sims-up.mjs ...
// Usage: node sims-up.mjs <text-out> <ports-json-out>
import { spawn } from "node:child_process";
import fs from "node:fs";
const [,, textOut, portsOut] = process.argv;
const words = ["sol","luna","mar","río","casa","perro","libro","calle","noche","día","árbol","agua"];
const paras = [];
for (let p = 0; p < 24; p++) paras.push(Array.from({ length: 12 + (p % 7) * 5 }, (_, i) => words[(p * 5 + i * 7) % words.length]).join(" ").replace(/^./, c => c.toUpperCase()) + ` ${p}x.`);
fs.writeFileSync(textOut, paras.join("\n\n"));
const sims = spawn(new URL("../../../target/debug/flts-e2e-sims", import.meta.url).pathname, { stdio: ["pipe", "pipe", "inherit"] });
let buf = "";
sims.stdout.on("data", async (c) => {
  buf += c;
  const line = buf.split("\n").find((l) => l.trim().startsWith("{"));
  if (!line || fs.existsSync(portsOut)) return;
  const ports = JSON.parse(line);
  const scripts = paras.map((text) => {
    const toks = text.slice(0, -1).split(" ");
    const wl = toks.map((w) => ({ o: w, t: ["en-" + w.toLowerCase()], n: null, p: false, g: { lf: w.toLowerCase(), lt: "en", pos: "common_noun", pl: null, pe: null, te: null, ca: null, ot: null } }));
    wl.push({ o: ".", t: ["."], n: null, p: true, g: { lf: ".", lt: ".", pos: "punctuation", pl: null, pe: null, te: null, ca: null, ot: null } });
    return { matchSubstring: text, translation: { s: [{ ft: "full", wl }] } };
  });
  await fetch(`http://127.0.0.1:${ports.llm}/_sim/seed`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ scripts }) });
  fs.writeFileSync(portsOut, JSON.stringify(ports));
});
