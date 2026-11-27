import { chromium } from "playwright";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium",
  headless: true,
  args: ["--no-sandbox", "--disable-dev-shm-usage"],
});
const page = await browser.newPage({
  viewport: { width: 1440, height: 1050 },
  deviceScaleFactor: 1,
});
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const requests = [];
page.on("request", (r) => {
  if (r.url().includes("/api/emissions?")) requests.push(r.url());
});
const base = process.env.BASE_URL || "http://127.0.0.1:3000";
const checks = [];
const ok = (label) => {
  checks.push(label);
  console.log("PASS", label);
};
try {
  await page.goto(base + "/?replay=1", { waitUntil: "networkidle" });
  await page.locator("#inspector").waitFor({ state: "visible" });
  assert.match(
    await page.locator("#inspector").innerText(),
    /Position undetermined/,
  );
  assert.equal(await page.locator(".rx-row").count(), 3);
  assert.equal(await page.locator(".leaflet-marker-icon .tx-icon").count(), 0);
  assert.match(
    await page.locator("#mode-badge").innerText(),
    /REAL RECORDING/,
  );
  ok(
    "real recording: ambiguous hypotheses, no unique transmitter marker, three receptions",
  );
  await page.screenshot({ path: "evidence/desktop.png", fullPage: true });
  const before = requests.length;
  await page.evaluate(() => map.setView([50.0123, 9.0331], 11));
  await page.waitForTimeout(600);
  assert.ok(requests.length > before);
  assert.equal(await page.locator("#emission-count").innerText(), "1");
  assert.equal(await page.locator("#inspector .outside").count(), 3);
  ok(
    "zoom queries viewport; region intersects even when centroid/receivers outside; selected receiver list preserved",
  );
  await page.evaluate(() => map.setView([30, 30], 6));
  await page.waitForTimeout(600);
  assert.equal(await page.locator("#emission-count").innerText(), "0");
  assert.equal(await page.locator(".rx-row").count(), 3);
  ok(
    "pan filters emissions while detail retains all confirmed receivers outside viewport",
  );
  await page.locator("#band").selectOption("20m");
  await page.waitForTimeout(400);
  assert.match(
    await page.locator("#coverage-title").innerText(),
    /Unmonitored interval/,
  );
  assert.equal(await page.locator("#emission-count").innerText(), "0");
  ok("band filter returns unmonitored, not no-emission");
  await page.locator("#reset").click();
  await page.locator("#inspector").waitFor({ state: "visible" });
  await page.locator("#end").fill("2020-08-13T06:00");
  await page.locator("#apply").click();
  assert.match(
    await page.locator("#filter-error").innerText(),
    /valid UTC interval/,
  );
  ok("invalid UTC interval rejected");
  await page.locator("#reset").click();
  await page.locator("#inspector").waitFor({ state: "visible" });
  await page.locator("#proof").click();
  await page.locator("#show-evaluation").click();
  await page.waitForFunction(
    () =>
      document.querySelector("#modal-title").textContent ===
      "Real-recording evaluation",
  );
  assert.match(await page.locator("#modal-content").innerText(), /1\.79 km/);
  assert.match(
    await page.locator("#modal-content").innerText(),
    /global ambiguity/,
  );
  ok(
    "post-hoc real-data error shown without choosing the known transmitter hypothesis",
  );
  await page.locator("#close-modal").click();
  await page.locator("#test-sync").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("#mode-badge")
        .textContent.includes("GNSS LOSS"),
    {},
    { timeout: 60000 },
  );
  await page.waitForFunction(() =>
    document.querySelector("#inspector").textContent.includes("NO GNSS"),
  );
  assert.equal(await page.locator("#emission-count").innerText(), "0");
  assert.match(
    await page.locator("#inspector").innerText(),
    /Position undetermined/,
  );
  assert.equal(await page.locator("#unlocated .emission-card").count(), 1);
  await page.screenshot({ path: "evidence/sync-loss.png", fullPage: true });
  ok(
    "loss-of-sync result from actual IQ opens correctly, with no position or candidate area",
  );
  await page.locator("#mode-live").click();
  await page.waitForTimeout(400);
  assert.match(await page.locator("#mode-badge").innerText(), /^LIVE$/);
  assert.match(
    await page.locator("#coverage-title").innerText(),
    /Unmonitored interval/,
  );
  await page.locator("#replay").click();
  await page.locator("#modal").waitFor({ state: "visible" });
  assert.match(
    await page.locator("#modal-content").innerText(),
    /Administrator token|No SDR operator/,
  );
  await page.locator("#close-modal").click();
  ok("live and replay separated; capture requires administrator token");
  const denied = await page.request.post(base + "/api/capture", {data: {target_id: "dcf77-validation"}});
  assert.ok([401,403].includes(denied.status()));
  const forced = await page.request.post(base + "/api/replay", {data: {dataset_id: "dcf77-20200813", force: true}});
  assert.ok([401,403].includes(forced.status()));
  ok("unauthenticated capture and forced DSP refused before starting work");
  const catalog = await (await page.request.get(base + "/api/sources?limit=2")).json();
  if (catalog.available) {
    assert.ok(catalog.total > 100);
    assert.ok(catalog.receivers.every(r => r.reception_confirmed === false));
    await page.locator("#nav-sources").click();
    await page.locator("#catalog-form").waitFor();
    await page.locator("#catalog-search").fill("0cb2b7d6cd86");
    await page.locator("#catalog-form button").click();
    await page.waitForFunction(() => document.querySelectorAll("[data-source-id]").length === 1 && document.querySelector("#modal-content").textContent.includes("PE1NOD"));
    assert.equal(await page.locator("[data-source-id]").count(),1);
    await page.locator("#close-modal").click();
    ok("real public catalog searchable, policy exclusions retained, metadata never presented as RF reception");
  }
  const ready = await page.request.get(base + "/api/ready");
  assert.equal(ready.status(),200);
  assert.match(ready.headers()["content-security-policy"],/frame-ancestors 'none'/);
  assert.equal(ready.headers()["x-content-type-options"],"nosniff");
  ok("readiness and browser security headers present");
  await page.locator("#mode-replay").click();
  await page.locator("#inspector").waitFor({ state: "visible" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(500);
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  );
  await page.screenshot({ path: "evidence/mobile.png", fullPage: true });
  await page.locator("#proof").click();
  assert.ok(await page.locator("#modal").isVisible());
  await page.locator("#close-modal").click();
  ok("390px responsive layout and modal interaction; no horizontal overflow");
  assert.deepEqual(errors, []);
  ok("no browser JavaScript exceptions");
  writeFileSync(
    "evidence/browser-tests.json",
    JSON.stringify(
      {
        passed: true,
        checks,
        console_errors: errors,
        browser: await browser.version(),
        utc: new Date().toISOString(),
      },
      null,
      2,
    ),
  );
} catch(error) {
  writeFileSync("evidence/browser-tests.json",JSON.stringify({passed:false,checks,error:error.message,console_errors:errors,utc:new Date().toISOString()},null,2));
  throw error;
} finally {
  await browser.close();
}
