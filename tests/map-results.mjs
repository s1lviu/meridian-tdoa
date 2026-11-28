import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// Replay the archived two-region result only through intercepted responses in this browser.
export async function checkMapResults(browser, base, evidence, mobile) {
  const archive = JSON.parse(readFileSync(new URL("../evidence/replay-result.json", import.meta.url)));
  const viewport = mobile ? { width: 390, height: 844 } : { width: 1440, height: 950 };
  const now = new Date().toISOString();
  const page = await browser.newPage({ viewport, isMobile: mobile, hasTouch: mobile, reducedMotion: mobile ? "reduce" : "no-preference" });
  try {
    const errors = [], globeRequests = [];
    page.on("pageerror", e => errors.push(e.message));
    page.on("request", r => { if (/\/(globe\.js|d3-[^/]+\.js)$/.test(new URL(r.url()).pathname)) globeRequests.push(r.url()); });
    await page.addInitScript(() => {
      window.selectionAnimations = 0;
      const animate = Element.prototype.animate;
      Element.prototype.animate = function(...args) {
        if (this.id === "inspector") window.selectionAnimations++;
        return animate.apply(this, args);
      };
    });
    const observation = {
      ...archive, profile: "dcf77_prn_v1", confirmed_count: 3, pair_count: archive.pairs.length,
      accepted_pair_count: archive.pairs.filter(p => p.accepted).length,
    };
    const heard = Array.from({ length: 120 }, (_, i) => ({
      id: "heard-" + i, identity: { callsign: "HEARD-" + i }, profile: "ft8_v1",
      frequency_hz: 14074000, start_utc: now, estimate: null, hypotheses: [],
      confirmed_count: 1, pair_count: 0, accepted_pair_count: 0,
    }));
    const tracks = [];
    function addTrack(name, lat, lon, radius = 5) {
      tracks.push({ track: { key: name, callsign: name, profile: "ft8_v1", frequency_hz: 14074000, band: "20m",
        center: { lat, lon }, observations: [name + "-obs"], fixes: 1, rejected: 0, first_utc: now, last_utc: now },
        ellipse_95: { major_km: radius, minor_km: radius / 2, azimuth_deg: 45 } });
    }
    addTrack("SAME-A", 50.2, 8.5); addTrack("SAME-B", 50.7, 9.7);
    const centers = [[44, 18], [38, -96], [33, 134], [-30, 144]];
    for (let i = 0; i < 60; i++) {
      const [lat, lon] = centers[i % centers.length];
      addTrack("TEST-" + i, lat + Math.floor(i / 4) % 4, lon + Math.floor(i / 16) * 3);
    }
    addTrack("TEST-60", -43, -73); addTrack("TEST-61", -1, 37);
    addTrack("TEST-62", 63, -119); addTrack("TEST-63", 31, -18);
    addTrack("WIDE", 30, -20, 10000);
    const emissions = [observation];
    await page.route("**/api/emissions?**", route => route.fulfill({ json: {
      emissions, unlocated: heard, monitoring: { segments: [...emissions, ...heard].map(e => ({ start_utc: e.start_utc })) },
    } }));
    await page.route("**/api/emissions/" + archive.id, route => route.fulfill({ json: observation }));
    await page.route("**/api/emissions/" + archive.id + "/evaluation", route => route.fulfill({ status: 404, json: {} }));
    await page.route("**/api/tracks?**", route => route.fulfill({ json: { tracks } }));
    await page.route("**/api/tracks/*", route => {
      const key = decodeURIComponent(new URL(route.request().url()).pathname.split("/").at(-1));
      return route.fulfill({ json: { ...tracks.find(t => t.track.key === key), observations: [] } });
    });
    await page.goto(base);
    await page.waitForFunction(() => document.querySelector("#heard-count").textContent === "120");
    assert.equal(await page.locator("#ambiguous-count").textContent(), "1");
    assert.equal(await page.locator("#located-count").textContent(), String(tracks.length));
    assert.equal(await page.locator("#show-observations").isChecked(), false);
    assert.equal(await page.locator("#map-key summary").isVisible(), true);
    assert.equal(await page.locator("#wide-group").isVisible(), true);
    assert.ok(await page.locator(".overview-marker").count() < tracks.length);
    const overlaps = selector => page.evaluate(selector => {
      const boxes = [...document.querySelectorAll(selector)].filter(el => getComputedStyle(el).visibility === "visible").map(el => el.getBoundingClientRect());
      return { count: boxes.length, collision: boxes.some((a, i) => boxes.slice(i + 1).some(b => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top)) };
    }, selector);
    assert.ok((await overlaps(".tx-label")).count > 0);
    assert.equal((await overlaps(".tx-label, .area-count, .area-symbol")).collision, false);
    await page.screenshot({ path: join(evidence, mobile ? "mobile-overview.png" : "desktop-overview.png"), animations: "disabled" });
    await page.locator("#wide-group").click();
    assert.equal(await page.locator("#group-list button").filter({ hasText: "WIDE" }).count(), 1);
    await page.locator("#inspector-close").click();
    await page.locator("#search").fill("SAME");
    assert.equal(await page.locator("#located-count").textContent(), "2");
    const group = page.getByRole("button", { name: "2 located emitters in this area" });
    const view = () => page.evaluate(() => [map.getCenter().lat, map.getCenter().lng, map.getZoom()]);
    const opening = await view();
    if (mobile) await group.tap(); else { await group.focus(); await group.press("Enter"); }
    assert.equal(await page.locator("#group-list button").count(), 2);
    await page.locator("#group-list button").filter({ hasText: "SAME-B" }).click();
    await page.waitForFunction(() => document.querySelector("#inspector h3")?.textContent === "SAME-B");
    assert.equal(await page.locator(".selected-track").count(), 1);
    assert.match(await page.locator("#inspector").innerText(), /rounded outward/);
    await page.locator("#inspector-close").click();
    const restored = await view();
    assert.ok(restored.every((v, i) => Math.abs(v - opening[i]) < 0.001), JSON.stringify({ opening, restored }));
    await page.evaluate(() => map.setView([50.5, 9], map.getMaxZoom(), { animate: false }));
    await page.locator("#wide-group").click();
    assert.equal(await page.locator("#group-list button").count(), 2);
    await page.locator("#inspector-close").click();
    await page.evaluate(([lat, lon, zoom]) => map.setView([lat, lon], zoom, { animate: false }), opening);
    await page.locator("#search").fill("");
    await page.locator("#heard-more").click();
    await page.locator("#heard-more").click();
    assert.equal(await page.locator("#heard-list button").filter({ hasText: "HEARD-119" }).count(), 1);
    const candidate = page.locator("#ambiguous-list button").filter({ hasText: "DCF77" });
    await candidate.focus();
    await page.evaluate(() => refresh());
    assert.equal(await candidate.evaluate(el => el === document.activeElement), true);
    await candidate.click();
    await page.locator("#inspector .status-pill").waitFor();
    assert.equal(await page.locator(".selected-hypothesis").count(), 2);
    assert.equal(await page.locator(".rx-icon").count(), 3);
    assert.equal(await page.locator(".focus-tag.hypothesis").allTextContents().then(names => names.join(",")), "A,B");
    assert.equal(await page.evaluate(() => window.selectionAnimations > 0), !mobile);
    const before = await view();
    heard.push({ ...heard[0], id: "new", identity: { callsign: "NEW" } });
    await page.evaluate(() => refresh());
    assert.equal(await page.locator("#new-observations").innerText(), "New observations · 1");
    assert.deepEqual(await view(), before);
    assert.equal(await page.locator("#inspector .status-pill").innerText(), "Position undetermined");
    assert.equal((await overlaps(".focus-tag, .leaflet-control-scale, .leaflet-control-zoom")).collision, false);
    await page.screenshot({ path: join(evidence, mobile ? "mobile-ambiguous.png" : "desktop-ambiguous.png"), animations: "disabled" });
    assert.deepEqual(globeRequests, []);
    await page.locator("#open-globe").click();
    await page.getByRole("button", { name: "Region B · far side" }).waitFor();
    await page.getByRole("button", { name: "Region B · far side" }).click();
    await page.getByRole("button", { name: "Region A · far side" }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Region B · visible" }).isVisible(), true);
    await page.locator("#globe-flat").click();
    assert.equal(await page.locator(".selected-hypothesis").count(), 2);
    assert.equal(await page.evaluate(() => state.selected.areas.every(a => map.getBounds().contains(L.latLngBounds(visibleRing(a))))), true);
    await page.locator("#map-key summary").click();
    await page.locator("#show-observations").check();
    await page.locator("#show-receivers").uncheck();
    await page.locator("#show-observations").uncheck();
    assert.equal(await page.locator(".selected-hypothesis").count(), 2);
    assert.equal(await page.locator(".rx-icon").count(), 0);
    await page.locator("#map-key summary").click();
    await page.locator("#new-observations").click();
    assert.equal(await page.locator("#heard-count").innerText(), "121");
    assert.equal(await candidate.evaluate(el => el === document.activeElement), true);
    const crossing = { ...observation, id: "date-line", identity: { callsign: "DATE-LINE" }, profile: "ft8_v1", hypotheses: [179.5, -179.5].map(lon => ({
      ...archive.hypotheses[0], center: { lat: 25, lon }, polygon: [[24.8, lon-0.3], [25.2, lon-0.3], [25.2, lon+0.3], [24.8, lon+0.3]].map(([lat, lon]) => ({ lat, lon })),
    })) };
    emissions.push(crossing);
    await page.route("**/api/emissions/date-line", route => route.fulfill({ json: crossing }));
    await page.route("**/api/emissions/date-line/evaluation", route => route.fulfill({ status: 404, json: {} }));
    await page.evaluate(() => refresh());
    await page.locator("#ambiguous-list button").filter({ hasText: "DATE-LINE" }).click();
    await page.getByRole("heading", { name: "DATE-LINE" }).waitFor();
    assert.equal(await page.evaluate(() => map.getZoom() > 5 && state.selected.areas.every(a => map.getBounds().contains(L.latLngBounds(visibleRing(a))))), true);
    assert.equal((await overlaps(".focus-tag")).collision, false);
    assert.deepEqual(errors, []);
  } finally {
    await page.close();
  }
}
