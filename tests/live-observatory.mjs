// Read-only acceptance against a running monitor with two real verified observations.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";

const base = process.env.BASE_URL || "http://127.0.0.1:3000";
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const writes = [], errors = [];
const pinned = await browser.newPage();
for (const tab of [page, pinned]) {
  tab.on("request", (r) => { if (r.method() !== "GET") writes.push(r.method() + " " + r.url()); });
  tab.on("pageerror", (e) => errors.push(e.message));
}
const selected = () => page.locator('.history-fix[aria-pressed="true"]').getAttribute("data-fix");
try {
  await page.goto(base);
  const multi = await page.locator("#beacon-network").isVisible() ||
    (await (await page.request.get(base + "/api/info")).json()).targets.some(t => t.verification === "ncdxf_cw_v1");
  if (multi) {
    await page.locator(".band-channel").nth(4).waitFor();
    assert.equal(await page.locator(".band-channel").count(), 5);
    await page.locator("#roster-count").waitFor({ state: "attached" });
    const transmitter = page.locator("#located-stations button").filter({hasText:"DCF77"});
    await transmitter.waitFor();
    assert.equal(await page.locator("#show-reception").isChecked(), false);
    assert.equal(await page.locator(".heard-site, .digital-field").count(), 0);
    assert.equal(await page.evaluate(() => pool.getLayers().length + directoryOnly.getLayers().length), 0);
    await transmitter.click();
    await page.locator(".tx-coordinates").waitFor();
    await page.waitForFunction(() => isDcf(state.selected) && map.getZoom() > 3 && map.getBounds().contains([state.selected.estimate.center.lat,state.selected.estimate.center.lon]));
    mkdirSync("evidence", {recursive:true});
    await page.locator(".map-card").screenshot({path:"evidence/beacon-map.png"});
    await pinned.goto(base + "/?station=DCF77&band=20m");
    await pinned.locator("#station-notice").waitFor();
    assert.equal(await pinned.locator("#band").inputValue(), "20m");
    assert.match(await pinned.locator("#station-notice").innerText(), /Waiting for a confirmed reception of DCF77/);
    await pinned.goto(base + "/?station=DCF77");
    await pinned.locator(".tx-coordinates").waitFor();
    await pinned.evaluate(() => refresh());
    assert.equal(await pinned.evaluate(() => isDcf(state.selected)), true);
    assert.equal(await pinned.locator("#latest-fix").innerText(), "Following DCF77");
    const unresolved = page.locator("#unresolved-stations");
    if (await unresolved.isVisible()) {
      assert.equal(await unresolved.getAttribute("open"), "");
      await unresolved.locator("button").first().click();
      await page.waitForFunction(() => !!state.selected && !state.selected.estimate);
      assert.equal(await page.locator("#inspector .tx-coordinates").count(), 0);
      await unresolved.locator("summary").click();
    }
    await page.locator("#show-reception").check();
    await page.locator("#world-network").click();
    console.log("PASS measured transmitters first, one-click TDoA centre and uncertainty, live station link and unresolved evidence");
    await page.locator("#map-activity").click();
    assert.equal(await page.locator("#beacon-roster").getAttribute("open"), "");
    await page.locator(".roster-item").nth(17).waitFor();
    assert.equal(await page.locator(".roster-item").count(), 18);
    const sites = await page.evaluate(() => new Set(state.data.signals.filter(isIbp).flatMap(s => s.confirmed_receivers.map(r => r.id))).size);
    await page.waitForFunction(n => document.querySelectorAll(".heard-site").length === n, sites);
    assert.equal(await page.locator(".heard-site").count(), sites);
    const heard = page.locator("#roster-list [data-heard]").first();
    if (await heard.count()) {
      await heard.click();
      await page.waitForFunction(() => state.selected?.reference?.profile === "ncdxf_cw_v1");
      await page.locator("#proof").click();
      await page.getByRole("heading", {name:"CW verification in each receiver"}).waitFor();
      await page.locator("#show-evaluation").click();
      await page.getByRole("heading",{name:"Live observation evaluation"}).waitFor();
      assert.match(await page.locator("#modal-content").innerText(),/published locator/);
      await page.locator("#close-modal").click();
    }
    await page.locator("#beacon-roster summary").click();
    await page.locator('[data-band="17m"]').click();
    await page.waitForFunction(() => $("band").value === "17m" && state.data?.band_stats?.length === 6 && $("band-stats").querySelector(".band-selected")?.dataset.statsBand === "17m");
    const net = await (await page.request.get(base+"/api/beacons")).json();
    assert.equal(await page.evaluate(() => pool.getLayers().length), net.receiver_pool.filter(r => r.bands.includes("17m")).length);
    const outsideBand = net.receiver_pool.filter(r => !r.bands.includes("17m")).length;
    assert.equal(await page.evaluate(() => directoryOnly.getLayers().length), net.other_receivers.length + outsideBand);
    await page.locator("#show-directory").uncheck();
    assert.equal(await page.evaluate(() => directoryOnly.getLayers().length), 0);
    await page.locator("#show-directory").check();
    await page.locator("#show-pool").uncheck();
    assert.equal(await page.evaluate(() => pool.getLayers().length),0);
    await page.locator("#show-pool").check();
    await page.locator("#all-bands").click();
    await page.locator("#world-network").click();
    assert.ok(await page.evaluate(() => map.getZoom() <= 2));
    if (net.other_receivers.length) {
      const station = net.other_receivers.find(r => r.reason === "Insufficient directory GNSS status") ?? net.other_receivers[0];
      assert.ok(!net.receiver_pool.some(r => r.id === station.id));
      await pinned.goto(base + "/?view=activity");
      await pinned.locator("#map-activity").waitFor();
      await pinned.locator("#nav-sources").click();
      await pinned.locator("#catalog-search").fill(station.id);
      await pinned.locator("#catalog-form button").click();
      const card = pinned.locator(".dataset-card").filter({has: pinned.getByText(station.id, {exact:false})}).first();
      assert.ok(await card.locator("input[type=checkbox]").isDisabled());
      await card.getByRole("link", {name:"Show on map"}).click();
      const popup = pinned.locator(".receiver-popup");
      await popup.waitFor();
      assert.ok((await popup.innerText()).includes(station.reason));
      assert.ok((await popup.innerText()).includes(station.label));
      assert.equal(await pinned.evaluate(() => map.getZoom()), 8);
      assert.match(await pinned.locator("#directory-view").innerText(), /[1-9]\d* listed/);
      await pinned.locator("#band").selectOption("lf");
      assert.equal(await pinned.evaluate(() => pool.getLayers().length), 0);
      assert.equal(await pinned.evaluate(() => directoryOnly.getLayers().length), net.other_receivers.length + net.receiver_pool.length);
      await pinned.locator("#mode-replay").click();
      assert.equal(await pinned.evaluate(() => directoryOnly.getLayers().length), 0);
      console.log("PASS directory search, receiver deep link, exclusion reason, regional count and independent layer controls");
    }
    mkdirSync("evidence", {recursive:true});
    await page.screenshot({path:"evidence/beacon-network-world.png", fullPage:true});
    await page.setViewportSize({width:390,height:844});
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.setViewportSize({width:1440,height:1000});
    const grouping = await page.evaluate(() => {
      // Browser-only regression exercise. These fixtures never leave the test tab.
      const previousData=state.data, previousSelection=state.selected;
      const solution={center:{lat:1,lon:1},polygon:[{lat:0,lon:0},{lat:2,lon:0},{lat:1,lon:2},{lat:0,lon:0}]};
      function example(id,call,freq,time) {
        return {id,identity:{callsign:call},profile:"ncdxf_cw_v1",frequency_hz:freq,start_utc:`2026-01-01T00:00:${time}Z`,estimate:solution,hypotheses:[]};
      }
      const a=example("a","OH2B",14100000,"40"), otherBand=example("b","OH2B",18110000,"30"),
        otherCall=example("c","VK6RBP",14100000,"20"), prior=example("d","OH2B",14100000,"10");
      state.selected=null;
      state.data={emissions:[a,otherBand,otherCall,prior],unlocated:[],history:[a,otherBand,otherCall,prior],signals:[a,otherBand,otherCall]};
      renderRegions(state.data.emissions);
      const result=[previousReference(a)?.id, document.querySelectorAll(".tx-icon").length];
      const heardAgain = {...a, id:"later", estimate:null, start_utc:"2026-01-01T00:00:50Z", last_position:a};
      state.data.signals = [heardAgain, otherBand, otherCall];
      renderRegions(state.data.emissions);
      result.push(document.querySelectorAll(".tx-icon").length, [...document.querySelectorAll(".tx-label")].some(label => label.textContent.includes("last TDoA 00:00:40 UTC · heard again 00:00:50 UTC")));
      state.data.signals = [{...heardAgain, last_position:undefined}, otherBand, otherCall];
      renderRegions(state.data.emissions);
      result.push(document.querySelectorAll(".tx-icon").length);
      state.data=previousData;
      state.selected=previousSelection;
      state.regionKey=null;
      renderRegions(previousData.emissions);
      return result;
    });
    assert.deepEqual(grouping,["d",3,3,true,2], "A later reception retains only a measured result for the same callsign and band");
    console.log("PASS five live schedule channels, 18-beacon roster, read-only worldwide receiver layer, band filters and separate accuracy rows");
  }
  const config = await (await page.request.get(base + "/api/info")).json();
  if (config.digital_reports) {
    // A freshly started service may wait a full WSPR cycle for its first reports.
    await page.waitForFunction(async () => {
      const feed = await fetch("/api/activity").then(r => r.json());
      return feed.connected && feed.reporters > 8 &&
        ["FT8", "WSPR"].every(mode => feed.bands.some(row => row.mode === mode && row.reports > 0));
    }, undefined, {polling:3000, timeout:180000});
    const feed = await (await page.request.get(base + "/api/activity")).json();
    assert.equal(feed.enabled, true);
    assert.equal(feed.connected, true);
    assert.ok(feed.reporters > 8);
    assert.ok(["FT8","WSPR"].every(m => feed.bands.some(r => r.mode === m && r.reports > 0)));
    const row = feed.bands.find(r => r.band === "30m" && Object.keys(r.fields).length) || feed.bands.find(r => Object.keys(r.fields).length);
    assert.ok(row);
    await page.getByRole("button", {name:/FT8 \+ WSPR · .+ reports/}).click();
    assert.equal(await page.locator("#digital-bands").getAttribute("open"), "");
    await page.locator(`[data-report-band="${row.band}"]`).click();
    await page.waitForFunction(b => $("band").value === b && document.querySelector(".digital-field"), row.band);
    await page.locator("#digital-ft8").uncheck();
    await page.locator("#digital-wspr").uncheck();
    assert.equal(await page.locator(".digital-field").count(),0);
    assert.equal(await page.locator("#digital-legend").isVisible(),false);
    await page.locator("#digital-wspr").check();
    await page.locator("#digital-ft8").check();
    await page.locator("#follow-live").uncheck();
    assert.equal(await page.locator(".digital-field").count(),0);
    await page.locator("#follow-live").check();
    await page.locator("#all-bands").click();
    const field = Object.keys(row.fields).sort((a,b) => row.fields[b]-row.fields[a])[0];
    assert.match(field,/^[A-R]{2}$/);
    await pinned.goto(base + "/?field=" + field);
    await pinned.getByText("Reported transmitter field " + field, {exact:true}).waitFor();
    assert.ok(await pinned.locator(".receiver-popup").innerText().then(t => t.includes("20° × 10°")));
    await pinned.locator(".leaflet-popup-close-button").click();
    await pinned.locator(".leaflet-popup").waitFor({state:"detached"});
    await pinned.locator("#map").scrollIntoViewIfNeeded();
    const point = await pinned.evaluate(field => {
      const west = -180 + (field.charCodeAt(0) - 65) * 20;
      const south = -90 + (field.charCodeAt(1) - 65) * 10;
      const rect = map.getContainer().getBoundingClientRect();
      for (const dx of [10, 5, 15]) for (const dy of [5, 2, 8]) {
        const p = map.latLngToContainerPoint([south + dy, west + dx]);
        const x = rect.left + p.x, y = rect.top + p.y;
        if (document.elementFromPoint(x, y)?.classList.contains("digital-field")) return {x, y};
      }
    }, field);
    assert.ok(point, "The receiver layer must allow clicking the digital field underneath");
    await pinned.mouse.click(point.x, point.y);
    await pinned.getByText("Reported transmitter field " + field, {exact:true}).waitFor();
    await pinned.locator("#mode-replay").click();
    assert.equal(await pinned.locator("#digital-panel").isVisible(),false);
    assert.equal(await pinned.locator(".digital-field").count(),0);
    await page.locator("#digital-bands summary").click();
    console.log("PASS live FT8 and WSPR, >8 reporters, field links, band and mode filters, live UTC and replay isolation");
  }
  await page.locator("#band").selectOption("lf");
  await page.waitForFunction(() => isDcf(state.selected) && state.data.history.every(isDcf));
  const fixes = page.locator(".history-fix");
  await fixes.nth(1).waitFor();
  const list = await page.evaluate(() => state.data);
  assert.ok(list.observatory?.interval_s > 0);
  const [newest, previous] = await fixes.evaluateAll((b) => b.slice(0,2).map((x) => x.dataset.fix));
  await pinned.goto(base);
  await pinned.locator("#band").selectOption("lf");
  await pinned.waitForFunction(() => isDcf(state.selected) && state.data.history.every(isDcf));
  await pinned.locator(`[data-fix="${previous}"]`).click();
  await pinned.getByRole("button", { name: "Follow latest", exact: true }).waitFor();
  assert.equal(await selected(), newest);
  await page.locator("#monitor-clock").waitFor();
  await fixes.nth(1).click();
  await page.waitForFunction((id) => document.querySelector('.history-fix[aria-pressed="true"]')?.dataset.fix === id, previous);
  assert.equal(await page.locator("#latest-fix").innerText(), "Follow latest");
  await page.locator("#latest-fix").click();
  await page.waitForFunction((id) => document.querySelector('.history-fix[aria-pressed="true"]')?.dataset.fix === id, newest);
  const candidate = list.history.find((e) => e.used_ids.length > 3);
  assert.ok(candidate, "A verified capture with at least four contributing receivers is needed");
  await page.locator(`[data-fix="${candidate.id}"]`).click();
  const e = await (await page.request.get(base + "/api/emissions/" + candidate.id)).json();
  await page.locator("#three-rx").click();
  const n = e.reference.three_receivers.solutions.length;
  assert.equal(await page.locator(".tx-icon").count(), Number(n === 1));
  assert.equal(await page.locator(".observation-contour").count(), n);
  assert.equal(await page.locator(".prior-contour").count(), 0);
  assert.match(await page.locator("#fix-change").innerText(), /Same UTC samples/);
  await page.locator("#all-rx").click();
  assert.equal(await page.locator(".tx-icon").count(), Number(!!e.estimate));
  mkdirSync("evidence", { recursive: true });
  if (e.estimate) {
    await page.locator("#fit-estimates").click();
    assert.equal(await page.locator(".prior-contour").count(), Number(!!list.history[list.history.findIndex((r) => r.id === e.id) + 1]?.estimate));
    await page.screenshot({ path: "evidence/live-observatory-contours.png" });
  }
  await page.locator("#proof").click();
  await page.locator("#show-evaluation").click();
  await page.getByRole("heading", { name: "Live observation evaluation" }).waitFor();
  assert.match(await page.locator("#modal-content").innerText(), /LF · DCF77/);
  await page.locator("#close-modal").click();
  await page.locator("#latest-fix").click();
  await page.getByRole("button", { name: "Following latest" }).waitFor();
  await page.locator("#fit").click();
  await page.waitForFunction(() => map.getZoom() <= 7 && document.querySelector("#clock").textContent.includes(":"));
  await page.screenshot({ path: "evidence/live-observatory-desktop.png", fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({ path: "evidence/live-observatory-mobile.png", fullPage: true });
  console.log("PASS real captures, pin and follow, measured receiver subset, post-hoc LF evaluation, desktop and mobile");
  if (multi) {
    await page.locator("#all-bands").click();
    await page.waitForFunction(() => $("band").value === "all" && !!state.data?.observatory?.last_job);
    const before = await page.evaluate(() => state.data.observatory.last_job.id);
    await page.waitForFunction(id => state.data?.observatory?.last_job?.id !== id, before,
      { timeout: (list.observatory.interval_s * 2 + 60) * 1000 });
    const monitor = await page.evaluate(() => state.data.observatory);
    if (monitor.parallel_bands > 1 && monitor.parallel_bands * 2 <= 1 + monitor.additional_target_ids.length) {
      assert.equal(monitor.last_jobs.length, monitor.parallel_bands);
      assert.equal(monitor.next_target_ids.length, monitor.parallel_bands);
      assert.ok(monitor.last_jobs.every(j => !monitor.next_target_ids.includes(j.target_id)), "Each window advances past all just-scheduled bands");
    }
    assert.equal(await pinned.locator('.history-fix[aria-pressed="true"]').getAttribute("data-fix"), previous);
    const latest = page.locator(".history-fix").first();
    if (await latest.count()) assert.equal(await latest.getAttribute("aria-pressed"),"true");
    console.log("PASS shared multiband worker advanced with two visitors, follow and pinned views stayed independent");
  } else {
  // Wait for a new, genuinely verified window. The browser only reads the API.
  const before = await selected();
  await page.waitForFunction((id) => {
    const latest = document.querySelector(".history-fix");
    return latest && latest.dataset.fix !== id && latest.getAttribute("aria-pressed") === "true";
  }, before, { timeout: (list.observatory.interval_s * 2 + 60) * 1000 });
  console.log("PASS automatically followed the next verified capture", before, "→", await selected());
  await pinned.waitForFunction((id) => document.querySelector(".history-fix")?.dataset.fix === id, await selected());
  assert.equal(await pinned.locator('.history-fix[aria-pressed="true"]').getAttribute("data-fix"), previous);
  console.log("PASS second visitor stayed pinned as the new window arrived");
  }
  assert.deepEqual(errors, []);
  assert.deepEqual(writes, []);
} finally {
  await browser.close();
}
