// Browser checks of the map against a real server process started here, on controlled data:
// the bundled 2020 DCF77 recording (replay) and simulated own-node detections (live).
// Usage: CHROMIUM_PATH=… node tests/browser.mjs path/to/meridian-tdoa
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repo = resolve(new URL("..", import.meta.url).pathname);
const binary = resolve(process.argv[2] || "target/debug/meridian-tdoa");
const work = mkdtempSync(join(tmpdir(), "meridian-browser-"));
symlinkSync(join(repo, "web"), join(work, "web"));
mkdirSync(join(work, "data/results"), { recursive: true });
cpSync(join(repo, "data/recordings"), join(work, "data/recordings"), { recursive: true });
for (const f of ["dcf77-20200813.json", "dcf77-20200813-sync-loss.json"]) cpSync(join(repo, "data/results", f), join(work, "data/results", f));
const nodes = { n0: [44.4, 26.1], n1: [44.55, 25.8], n2: [44.2, 25.9], n3: [44.45, 26.45], n4: [44.25, 26.3] };
const tokens = Object.fromEntries(Object.keys(nodes).map((n) => [n, `token-${n}-${"y".repeat(24)}`]));
const config = JSON.parse(readFileSync(join(repo, "config/app.json"), "utf8"));
config.nodes = Object.entries(nodes).map(([id, [lat, lon]]) => ({
  id, label: `simulated ${id}`, position: { lat, lon }, disciplined: true,
  position_evidence: { url: "https://example.org/simulated", note: "simulated node" },
  token_sha256: createHash("sha256").update(tokens[id]).digest("hex"),
}));
writeFileSync(join(work, "config.json"), JSON.stringify(config));
const port = 3900 + Math.floor(Math.random() * 90);
const base = `http://127.0.0.1:${port}`;
const server = spawn(binary, [], { cwd: work, env: { ...process.env, MERIDIAN_CONFIG: "config.json", PORT: String(port), MERIDIAN_DIRECTORY_OFFLINE: "1" }, stdio: "ignore" });
const checks = [];
const ok = (label) => { checks.push(label); console.log("PASS", label); };
const C = 299792.458;
const dist = ([a, b], [c, d]) => {
  const p = (a * Math.PI) / 180, q = (c * Math.PI) / 180;
  const h = Math.sin((q - p) / 2) ** 2 + Math.cos(p) * Math.cos(q) * Math.sin((((d - b) * Math.PI) / 180) / 2) ** 2;
  return 6371.0088 * 2 * Math.asin(Math.min(1, Math.sqrt(h)));
};
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined, headless: true, args: ["--no-sandbox"] });
try {
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(base + "/api/ready")).ok) break; } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  const page = await browser.newPage({ viewport: { width: 1440, height: 950 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));

  // Replay: the real 2020 recording has three receivers and therefore two mirror hypotheses.
  const response = await page.goto(base + "/?replay=1", { waitUntil: "load" });
  assert.match(response.headers()["content-security-policy"], /script-src 'self'/);
  await page.locator("#inspector").waitFor({ state: "visible" });
  assert.match(await page.locator("#mode-badge").innerText(), /REAL RECORDING/);
  assert.match(await page.locator("#inspector").innerText(), /Position undetermined/);
  assert.equal(await page.locator("#inspector .rx-row").count(), 3);
  assert.equal(await page.locator("path.hypothesis-region").count(), 2);
  assert.equal(await page.locator("path.fix-region").count(), 0);
  ok("recording: two mirror hypotheses, no unique fix, three receivers listed");
  await page.screenshot({ path: join(repo, "evidence/desktop.png") });

  await page.locator("#sync-loss").click();
  await page.waitForFunction(() => document.querySelector("#inspector")?.innerText.includes("Fewer than three"));
  assert.equal(await page.locator("path.hypothesis-region").count(), 0);
  ok("GNSS-loss replay: receiver excluded, no region drawn");

  // Live: five GNSS-disciplined nodes report one target; the server locates and registers it.
  const target = [44.36, 26.2];
  const now = Date.now() / 1000;
  for (const [id, p] of Object.entries(nodes)) {
    const r = await fetch(base + "/api/detections", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer " + tokens[id] },
      body: JSON.stringify({ frequency_hz: 145.8e6, detections: [{ node: id, signal: "beacon-42", local_s: now + dist(target, p) / C, sigma_s: 1e-7 }] }),
    });
    assert.equal(r.status, 200);
  }
  await page.locator("#mode-live").click();
  await page.waitForFunction(() => document.querySelectorAll("#track-list li").length > 0, null, { timeout: 15000 });
  await page.locator("#track-list li button").first().click();
  await page.locator("#inspector").waitFor({ state: "visible" });
  assert.match(await page.locator("#inspector").innerText(), /Fused fixes/);
  assert.ok((await page.locator("path.leaflet-interactive").count()) >= 1);
  ok("live: own-node detections located, registered as a track and inspectable");
  await page.locator("#inspector .link").first().click();
  try {
    await page.waitForFunction(() => /pair delays/i.test(document.querySelector("#inspector")?.innerText || ""), null, { timeout: 8000 });
  } catch (e) {
    console.log("INSPECTOR:", await page.locator("#inspector").innerText());
    console.log("TOAST:", await page.locator("#toast").innerText().catch(() => ""));
    throw e;
  }
  assert.match(await page.locator("#inspector").innerText(), /Located/);
  assert.equal(await page.locator("#inspector .rx-row").count(), 5);
  ok("observation inspector: located, five receivers, pair delays");
  await page.screenshot({ path: join(repo, "evidence/live.png") });

  const mobile = await browser.newPage({ viewport: { width: 390, height: 844 }, isMobile: true });
  await mobile.goto(base + "/", { waitUntil: "load" });
  const overflow = await mobile.evaluate(() => document.scrollingElement.scrollWidth - window.innerWidth);
  assert.ok(overflow <= 0, `horizontal overflow ${overflow}px`);
  await mobile.screenshot({ path: join(repo, "evidence/mobile.png") });
  ok("390 px layout without horizontal overflow");
  assert.deepEqual(errors, []);
  ok("no page errors");
  writeFileSync(join(repo, "evidence/browser-tests.json"), JSON.stringify({ executed_at: new Date().toISOString(), checks }, null, 1));
} finally {
  await browser.close();
  server.kill("SIGTERM");
  rmSync(work, { recursive: true, force: true });
}
