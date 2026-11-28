// Browser checks of the live-only map against a real server process started here, on
// controlled data: simulated own-node detections and a stubbed scheduler state.
// Usage: CHROMIUM_PATH=… node tests/browser.mjs path/to/meridian-tdoa
// Screenshots and the check list go to evidence/ unless MERIDIAN_EVIDENCE_DIR names another directory.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { checkMapResults } from "./map-results.mjs";

const repo = resolve(new URL("..", import.meta.url).pathname);
const binary = resolve(process.argv[2] || "target/debug/meridian-tdoa");
const evidence = resolve(process.env.MERIDIAN_EVIDENCE_DIR || join(repo, "evidence"));
mkdirSync(evidence, { recursive: true });
const work = mkdtempSync(join(tmpdir(), "meridian-browser-"));
symlinkSync(join(repo, "web"), join(work, "web"));
mkdirSync(join(work, "data/results"), { recursive: true });
const nodes = { n0: [44.4, 26.1], n1: [44.55, 25.8], n2: [44.2, 25.9], n3: [44.45, 26.45], n4: [44.25, 26.3] };
const tokens = Object.fromEntries(Object.keys(nodes).map((n) => [n, `token-${n}-${"y".repeat(24)}`]));
const config = JSON.parse(readFileSync(join(repo, "config/app.json"), "utf8"));
config.nodes = Object.entries(nodes).map(([id, [lat, lon]]) => ({
  id, label: `simulated ${id}`, position: { lat, lon }, disciplined: true,
  position_evidence: { url: "https://example.org/simulated", note: "simulated node" },
  token_sha256: createHash("sha256").update(tokens[id]).digest("hex"),
}));
const available = createServer();
await new Promise((resolve, reject) => available.once("error", reject).listen(0, "127.0.0.1", resolve));
const port = available.address().port;
await new Promise((resolve) => available.close(resolve));
config.listen = `127.0.0.1:${port}`;
writeFileSync(join(work, "config.json"), JSON.stringify(config));
const base = `http://${config.listen}`;
const env = { ...process.env, MERIDIAN_CONFIG: "config.json", MERIDIAN_DIRECTORY_OFFLINE: "1" };
delete env.PORT;
const server = spawn(binary, [], { cwd: work, env, stdio: "ignore" });
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
  assert.equal(await (await fetch(base + "/api/monitor")).json(), null, "no scheduler configured");
  const crawler = await browser.newPage({ javaScriptEnabled: false });
  await crawler.goto(base);
  const tags = await crawler.locator("head meta[content]").evaluateAll(elements =>
    Object.fromEntries(elements.map(el => [el.getAttribute("property") || el.name, el.content])));
  const canonical = new URL(await crawler.locator('link[rel="canonical"]').getAttribute("href"));
  const socialImage = new URL(tags["og:image"]);
  const site = JSON.parse(await crawler.locator('script[type="application/ld+json"]').textContent());
  assert.equal(canonical.protocol, "https:");
  assert.equal(tags["og:url"], canonical.href);
  assert.equal(site["@type"], "WebSite");
  assert.equal(site.url, canonical.href);
  assert.equal(site.name, tags["og:site_name"]);
  assert.equal(socialImage.origin, canonical.origin);
  assert.equal(tags["twitter:card"], "summary_large_image");
  assert.equal(tags["twitter:image"], socialImage.href);
  for (const key of ["description", "og:title", "og:description", "og:image:alt", "twitter:image:alt"]) {
    assert.ok(tags[key]?.trim(), key);
  }
  assert.ok(await crawler.getByRole("heading", { name: "Live radio transmitter map" }).isVisible());
  assert.equal(await crawler.locator(".layout").isVisible(), false);
  // Request the published asset path from this test server, keeping CI offline.
  const imageResponse = await crawler.goto(new URL(socialImage.pathname, base).href);
  assert.equal(imageResponse.status(), 200);
  assert.equal(imageResponse.headers()["content-type"], tags["og:image:type"]);
  assert.ok((await imageResponse.body()).length < 600_000);
  const dimensions = await crawler.locator("img").evaluate(async img => {
    await img.decode();
    return [img.naturalWidth, img.naturalHeight];
  });
  assert.deepEqual(dimensions, [1200, 630]);
  assert.deepEqual(dimensions, [Number(tags["og:image:width"]), Number(tags["og:image:height"])]);
  assert.ok((await (await fetch(base + "/robots.txt")).text()).includes("Sitemap: " + new URL("sitemap.xml", canonical).href));
  assert.ok((await (await fetch(base + "/sitemap.xml")).text()).includes(`<loc>${canonical.href}</loc>`));
  await crawler.close();
  ok("search metadata and share image are readable without JavaScript and match the served files");

  const page = await browser.newPage({ viewport: { width: 1440, height: 950 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));

  // Countdown to the next scheduled window. The test server runs no scheduler, so a stub
  // serves its state; the page must count down, ask again once a window starts, and show a pause.
  const plan = (seconds, notice = null) => ({
    next_at: seconds == null ? null : new Date(Date.now() + seconds * 1000).toISOString(),
    next_target_ids: ["ft8-40m", "msf-channel"],
    notice,
  });
  let schedule = plan(65);
  await page.route("**/api/monitor", (route) => route.fulfill({ json: schedule }));

  // The map is live only: no archived recording is offered in the page or served by the API.
  const response = await page.goto(base + "/", { waitUntil: "load" });
  assert.match(response.headers()["content-security-policy"], /script-src 'self'/);
  await page.waitForFunction(() => document.querySelector("#stats-body")?.innerText.includes("No live observation"));
  assert.equal(await page.locator("#mode-replay, #replay-card, #sync-loss").count(), 0);
  assert.match(await page.locator("#mode-badge").innerText(), /^LIVE/);
  assert.deepEqual((await (await fetch(base + "/api/info")).json()).datasets, []);
  assert.match(await page.locator(".credit").innerText(), /^Made by Silviu YO6SAY/);
  ok("live only: no archived recording in the page or the API");

  // Live: five GNSS-disciplined nodes report one target; the server locates and registers it.
  await page.waitForFunction(() => /^Next window in 1:0\d · ft8-40m, msf-channel$/.test(document.querySelector("#next-window")?.textContent || ""));
  const left = async () => {
    const [m, s] = (await page.locator("#next-window b").innerText()).split(":").map(Number);
    return m * 60 + s;
  };
  const first = await left();
  await page.waitForFunction((before) => {
    const [m, s] = (document.querySelector("#next-window b")?.textContent || "9:99").split(":").map(Number);
    return m * 60 + s < before;
  }, first);
  ok("live: countdown to the next scheduled window");
  schedule = plan(-1);
  await page.evaluate(() => loadMonitor());
  await page.waitForFunction(() => (document.querySelector("#next-window")?.textContent || "").startsWith("Window starting"));
  schedule = plan(45);
  await page.waitForFunction(() => /^Next window in 0:4\d/.test(document.querySelector("#next-window")?.textContent || ""), null, { timeout: 8000 });
  ok("a started window makes the page ask for the following one");
  schedule = plan(null, "Monitor paused: test notice");
  await page.evaluate(() => loadMonitor());
  await page.waitForFunction(() => document.querySelector("#next-window.paused")?.textContent === "Monitor paused: test notice");
  schedule = plan(50);
  await page.evaluate(() => loadMonitor());
  await page.waitForFunction(() => /^Next window in 0:[45]\d/.test(document.querySelector("#next-window")?.textContent || ""));
  ok("a paused scheduler shows its notice instead of a countdown");
  await page.screenshot({ path: join(evidence, "desktop.png") });
  const target = [44.36, 26.2];
  async function sendBeacon(signal) {
    const now = Date.now() / 1000;
    for (const [id, p] of Object.entries(nodes)) {
      const r = await fetch(base + "/api/detections", {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: "Bearer " + tokens[id] },
        body: JSON.stringify({ frequency_hz: 145.8e6, detections: [{ node: id, signal, local_s: now + dist(target, p) / C, sigma_s: 1e-7 }] }),
      });
      assert.equal(r.status, 200);
    }
  }
  await sendBeacon("beacon-42");
  await page.waitForFunction(() => document.querySelectorAll("#track-list li").length > 0, null, { timeout: 15000 });
  await page.waitForFunction(() => /s \(1\)/.test(document.querySelector(".paint-delay")?.textContent || ""));
  ok("foreground live map measures RF-to-display latency from the hardware timestamp");
  await page.locator("#track-list li button").first().click();
  await page.locator("#inspector").waitFor({ state: "visible" });
  assert.match(await page.locator("#inspector").innerText(), /Fused fixes/);
  assert.equal(await page.locator("path.selected-track").count(), 1);
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
  await page.screenshot({ path: join(evidence, "live.png") });
  await page.locator("#inspector-close").click();
  await page.unroute("**/api/monitor");

  await sendBeacon("beacon-43");
  await page.waitForFunction(() => document.querySelector(".paint-delay")?.textContent.endsWith(" s (2)"));
  for (const column of [".api-delay", ".paint-delay"]) {
    const match = /^(\d+\.\d) \/ (\d+\.\d) s \(2\)$/.exec(await page.locator(column).innerText());
    assert.ok(match && Number(match[2]) >= Number(match[1]), `ordered percentiles and sample count in ${column}`);
  }
  await page.reload();
  await page.waitForFunction(() => document.querySelector(".api-delay")?.textContent.endsWith(" s (2)"));
  assert.equal(await page.locator(".paint-delay").innerText(), "—");
  ok("two live fixes have P50/P90 and counts, reloading keeps only measured API latency");

  const mobile = await browser.newPage({ viewport: { width: 390, height: 844 }, isMobile: true });
  await mobile.goto(base + "/", { waitUntil: "load" });
  await mobile.locator("#mode-badge").waitFor();
  const layout = await mobile.evaluate(() => {
    const right = (selector) => Math.round(document.querySelector(selector).getBoundingClientRect().right);
    return {
      width: innerWidth,
      scroll: document.scrollingElement.scrollWidth,
      panel: right(".panel"),
      badge: right("#mode-badge"),
      status: getComputedStyle(document.querySelector(".status-line")).display,
    };
  });
  // Content wider than the screen makes mobile Chromium widen the layout viewport itself, so
  // innerWidth alone hides the overflow: compare with the device width.
  assert.equal(layout.width, 390, JSON.stringify(layout));
  assert.ok(layout.scroll <= 390 && layout.panel <= 390 && layout.badge <= 390, JSON.stringify(layout));
  assert.notEqual(layout.status, "none");
  await mobile.screenshot({ path: join(evidence, "mobile.png") });
  ok("390 px layout: nothing wider than the screen, status line visible");
  for (const phone of [false, true]) {
    await checkMapResults(browser, base, evidence, phone);
    ok((phone ? "390 px" : "desktop") + ": paged results, grouped labels, focus and motion, both ambiguous regions, optional globe and date-line fit");
  }
  assert.deepEqual(errors, []);
  ok("no page errors");
  writeFileSync(join(evidence, "browser-tests.json"), JSON.stringify({ executed_at: new Date().toISOString(), checks }, null, 1));
} finally {
  await browser.close();
  server.kill("SIGTERM");
  rmSync(work, { recursive: true, force: true });
}
