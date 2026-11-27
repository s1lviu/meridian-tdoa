"use strict";
// Meridian map: emitters located from our own synchronized SDR captures (TDoA only).
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (n, d = 0) => (n == null || !isFinite(n) ? "—" : Number(n).toLocaleString("en-GB", { minimumFractionDigits: d, maximumFractionDigits: d }));
// Distances and times keep their significant digits from metres/nanoseconds to thousands.
const km = (v) => (v == null || !isFinite(v) ? "—" : v < 1 ? `${fmt(v * 1000)} m` : `${fmt(v, v < 10 ? 1 : 0)} km`);
const us = (v) => (v == null || !isFinite(v) ? "—" : Math.abs(v) < 1 ? `${fmt(v * 1000)} ns` : `${fmt(v, Math.abs(v) < 10 ? 2 : 0)} µs`);
const freq = (f) => (f >= 1e6 ? `${fmt(f / 1e6, 4)} MHz` : `${fmt(f / 1e3, f % 1000 ? 2 : 0)} kHz`);
const utc = (t) => (t ? new Date(t).toISOString().replace("T", " ").slice(0, 19) + " UTC" : "—");
const ago = (t) => {
  const s = (Date.now() - Date.parse(t)) / 1000;
  return s < 90 ? `${Math.round(s)} s ago` : s < 5400 ? `${Math.round(s / 60)} min ago` : `${fmt(s / 3600, 1)} h ago`;
};
const icons = () => window.lucide?.createIcons();
const PROFILE = { ft8_v1: "FT8", dcf77_prn_v1: "DCF77", correlated_v1: "Unknown", mlat_v1: "Node", ncdxf_cw_v1: "IBP" };

const state = { mode: "live", info: null, data: null, tracks: [], stats: [], statsStart: 0, selected: null, refreshing: false, pending: false, scenario: "normal" };
const map = L.map("map", { worldCopyJump: true, zoomSnap: 0.25, minZoom: 1, maxZoom: 14, zoomControl: false }).setView([35, 5], 2.5);
L.control.zoom({ position: "bottomright" }).addTo(map);
L.control.scale({ imperial: false, position: "bottomright" }).addTo(map);
const land = L.layerGroup().addTo(map);
const cities = L.layerGroup().addTo(map);
const fixes = L.layerGroup().addTo(map);
const tracksLayer = L.layerGroup().addTo(map);
const receivers = L.layerGroup().addTo(map);

fetch("/world.geojson")
  .then((r) => r.json())
  .then((g) => L.geoJSON(g, { interactive: false, style: { color: "#2a3a46", weight: 0.7, fillColor: "#17232c", fillOpacity: 1 } }).addTo(land))
  .catch(() => {});
let places = [];
fetch("/places.json").then((r) => (r.ok ? r.json() : [])).then((p) => { places = p; drawCities(); }).catch(() => {});
function drawCities() {
  cities.clearLayers();
  const z = map.getZoom();
  for (const [minZoom, name, lat, lon] of places) {
    if (z >= minZoom + 1.5) L.marker([lat, lon], { interactive: false, icon: L.divIcon({ className: "city", html: esc(name), iconSize: null }) }).addTo(cities);
  }
}
map.on("zoomend", drawCities);

async function api(path, options) {
  const r = await fetch(path, options);
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || `HTTP ${r.status}`);
  return body;
}
function notify(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(notify.timer);
  notify.timer = setTimeout(() => (t.hidden = true), 6000);
}
function setStatus(text, level = "ok") {
  const s = $("status");
  s.className = `status ${level}`;
  s.innerHTML = `<span class="dot"></span>${esc(text)}`;
}

// 95% ellipse from the registry (semi-axes in km, azimuth of the major axis from north).
function ellipse(center, major, minor, az) {
  const pts = [];
  const a = (az * Math.PI) / 180;
  const kmLat = 111.32;
  const kmLon = 111.32 * Math.max(0.05, Math.cos((center.lat * Math.PI) / 180));
  for (let i = 0; i <= 72; i++) {
    const t = (i / 72) * 2 * Math.PI;
    const x = major * Math.cos(t), y = minor * Math.sin(t);
    const east = x * Math.sin(a) + y * Math.cos(a);
    const north = x * Math.cos(a) - y * Math.sin(a);
    pts.push([Math.max(-85, Math.min(85, center.lat + north / kmLat)), center.lon + east / kmLon]);
  }
  return pts;
}
const polygon = (est) => est.polygon.map((p) => [p.lat, p.lon]);
const label = (e) => e.identity?.callsign || (e.profile === "dcf77_prn_v1" ? "DCF77" : PROFILE[e.profile] ? `${PROFILE[e.profile]} ${freq(e.frequency_hz)}` : freq(e.frequency_hz));
const trackName = (t) => t.callsign || (t.key === "ref:DCF77" ? "DCF77" : freq(t.frequency_hz));

// Four-character Maidenhead cell computed only from the measured fix.
function locatorCell(p) {
  const x = Math.floor((((p.lon + 180) % 360 + 360) % 360) / 2);
  const y = Math.min(179, Math.max(0, Math.floor(p.lat + 90)));
  const letters = String.fromCharCode(65 + Math.floor(x / 10), 65 + Math.floor(y / 10));
  const south = y - 90, west = 2 * x - 180;
  return { name: letters + (x % 10) + (y % 10), south, west, center: [south + 0.5, west + 1] };
}
const gridDisplay = (profile, call) => profile === "ft8_v1" || (profile === "mlat_v1" && !!call);
const positionText = (p, grid) => grid ? "Locator " + locatorCell(p).name : fmt(p.lat, 3) + "°, " + fmt(p.lon, 3) + "°";

// The displayed bounds cover the whole confidence region, rounded out to cell edges.
function displayRegion(points, center, grid) {
  if (!grid) return points;
  const cell = locatorCell(center);
  const lats = points.map(([lat]) => lat).concat(cell.south, cell.south + 1);
  const lons = points.map(([, lon]) => center.lon + ((lon - center.lon + 540) % 360) - 180);
  const west = Math.min(cell.west, 2 * Math.floor(Math.min(...lons) / 2));
  const east = Math.max(cell.west + 2, 2 * Math.ceil(Math.max(...lons) / 2));
  const south = Math.floor(Math.min(...lats)), north = Math.ceil(Math.max(...lats));
  return [[south, west], [north, west], [north, east], [south, east]];
}

function interval() {
  if (state.mode === "replay") {
    const d = state.info?.datasets?.[0];
    const t = Date.parse(d?.capture_utc || "2020-08-13T06:52:20Z");
    return [new Date(t - 60000).toISOString(), new Date(t + 120000).toISOString()];
  }
  const hours = Number($("window").value);
  return [new Date(Date.now() - hours * 3600e3).toISOString(), new Date(Date.now() + 60e3).toISOString()];
}

// Measure only newly drawn observations received while this live view is visible.
let viewScope = "", visibleSince = 0, lastDrawn = new Set();
const paintSamples = new Map();
const paintHistoryMs = 3600e3 * Math.max(...Array.from($("window").options, option => Number(option.value)));
function resetPaintWindow() {
  viewScope = "";
  visibleSince = Date.now();
  lastDrawn.clear();
}
document.addEventListener("visibilitychange", () => {
  resetPaintWindow();
  if (!document.hidden && state.mode === "live") refresh();
});
function measureMap(scope) {
  const current = new Set(), arrived = [];
  for (const {track} of state.tracks) {
    const id = track.observations.at(-1);
    const at = Date.parse(track.last_utc);
    current.add(id);
    if (scope !== viewScope || !track.rf_utc || at < visibleSince || lastDrawn.has(id)) continue;
    const hz = track.frequency_hz;
    const band = track.band || (hz < 300e3 ? "LF" : hz < 3e6 ? "MF" : hz < 30e6 ? "HF" : hz < 300e6 ? "VHF" : hz < 3e9 ? "UHF" : "SHF");
    arrived.push({profile: track.profile, band, at, id});
  }
  if (scope !== viewScope) visibleSince = Date.now();
  viewScope = scope;
  lastDrawn = current;
  if (!arrived.length || document.hidden) return;
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (scope !== viewScope || document.hidden) return;
    const seen = Date.now();
    for (const sample of arrived) {
      if (seen >= sample.at && sample.at >= visibleSince && !paintSamples.has(sample.id)) {
        paintSamples.set(sample.id, {...sample, seconds: (seen - sample.at) / 1000});
      }
    }
    drawStats();
  }));
}

async function refresh() {
  if (state.refreshing) { state.pending = true; return; }
  state.refreshing = true;
  try {
    const [start, end] = interval();
    const band = $("band").value;
    const profile = $("profile").value;
    const scope = [state.mode, band, profile, $("window").value].join("/");
    const q = `mode=${state.mode}&compact=true&start=${start}&end=${end}&band=${band}&scenario=${state.scenario}`;
    const tasks = [api(`/api/emissions?${q}`)];
    if (state.mode === "live") tasks.push(api(`/api/tracks?since=${start}&band=${band}`), api(`/api/stats?start=${start}&end=${end}`));
    const [data, tracks, stats] = await Promise.all(tasks);
    if (scope !== [state.mode, $("band").value, $("profile").value, $("window").value].join("/")) { state.pending = true; return; }
    state.data = data;
    state.tracks = (tracks?.tracks || []).filter((t) => profile === "all" || t.track.profile === profile || (profile === "dcf77_prn_v1" && t.track.key === "ref:DCF77"));
    drawTracks();
    drawFixes(profile);
    drawLists(profile);
    state.stats = stats?.rows || [];
    state.statsStart = Date.parse(start);
    drawStats();
    if (state.mode === "live") measureMap(scope);
    if (state.mode === "live") {
      const obs = data.monitoring?.segments?.length || 0;
      const next = data.observatory?.next_at ? ` · next window ${utc(data.observatory.next_at).slice(11, 19)}` : data.observatory?.notice ? ` · ${data.observatory.notice}` : "";
      setStatus(`${obs} observation(s) in window · ${state.tracks.length} located${next}`, state.stream === "open" ? "ok" : "warn");
    }
    if (state.mode === "replay" && !state.selected) {
      const first = data.emissions[0] || data.unlocated[0];
      if (first) selectObservation(first.id);
    }
  } catch (e) {
    setStatus("API unavailable", "warn");
    notify(e.message);
  } finally {
    state.refreshing = false;
    if (state.pending) { state.pending = false; refresh(); }
  }
}

function drawTracks() {
  tracksLayer.clearLayers();
  for (const { track: t, ellipse_95: el } of state.tracks) {
    const name = trackName(t);
    const grid = gridDisplay(t.profile, t.callsign);
    const region = displayRegion(ellipse(t.center, el.major_km, el.minor_km, el.azimuth_deg), t.center, grid);
    const shape = L.polygon(region, { color: "#5fd4a6", weight: 2, fillOpacity: 0.18 });
    shape.on("click", () => selectTrack(t.key));
    shape.bindTooltip(`${esc(name)} · ${t.fixes} fix(es) · ±${km(el.major_km)}`, { sticky: true });
    shape.addTo(tracksLayer);
    const marker = grid ? locatorCell(t.center).center : [t.center.lat, t.center.lon];
    if (!grid) L.circleMarker(marker, { radius: 4, color: "#e8fff7", weight: 1, fillColor: "#5fd4a6", fillOpacity: 1, className: "tx-point" }).on("click", () => selectTrack(t.key)).addTo(tracksLayer);
    L.marker(marker, { interactive: false, icon: L.divIcon({ className: "tx-label", html: esc(name), iconSize: null, iconAnchor: [-8, 6] }) }).addTo(tracksLayer);
  }
}

function drawFixes(profile) {
  fixes.clearLayers();
  if (!$("show-observations").checked || !state.data) return;
  for (const e of state.data.emissions) {
    if (profile !== "all" && e.profile !== profile) continue;
    for (const est of e.estimate ? [e.estimate] : e.hypotheses) {
      const region = displayRegion(polygon(est), est.center, gridDisplay(e.profile, e.identity?.callsign));
      const p = L.polygon(region, { color: e.estimate ? "#7cc7ff" : "#c79bff", weight: 1.2, dashArray: e.estimate ? null : "5 4", fillOpacity: 0.05, className: e.estimate ? "fix-region" : "hypothesis-region" });
      p.bindTooltip(`${esc(label(e))} · ${utc(e.start_utc)} · ${e.estimate ? "single fix" : "ambiguous"}`, { sticky: true });
      p.on("click", () => selectObservation(e.id));
      p.addTo(fixes);
    }
  }
}

function drawLists(profile) {
  const list = $("track-list");
  list.innerHTML = "";
  for (const { track: t, ellipse_95: el } of state.tracks) {
    const li = document.createElement("li");
    li.innerHTML = `<button><span><span class="call">${esc(trackName(t))}</span> <span class="meta">${esc(t.band || freq(t.frequency_hz))} · ${t.fixes} fix${t.fixes > 1 ? "es" : ""}</span></span><span class="meta">±${km(el.major_km)} · ${ago(t.last_utc)}</span></button>`;
    li.firstChild.onclick = () => selectTrack(t.key);
    list.append(li);
  }
  $("located-count").textContent = state.tracks.length;
  $("track-empty").hidden = state.tracks.length > 0;
  const heard = $("heard-list");
  heard.innerHTML = "";
  const rows = [...(state.data?.unlocated || []), ...(state.data?.emissions || []).filter((e) => !e.estimate)]
    .filter((e) => profile === "all" || e.profile === profile)
    .slice(0, 120);
  for (const e of rows) {
    const li = document.createElement("li");
    li.innerHTML = `<button><span><span class="call">${esc(label(e))}</span> <span class="meta">${e.confirmed_count} rx · ${e.accepted_pair_count}/${e.pair_count} pairs</span></span><span class="meta">${esc(utc(e.start_utc).slice(11, 19))}</span></button>`;
    li.firstChild.onclick = () => selectObservation(e.id);
    heard.append(li);
  }
  $("heard-count").textContent = rows.length;
}

const pct = (x) => x == null ? "—" : fmt(100 * x) + "%";

function latencyText(p50, p90, count) {
  return count > 0 ? `${fmt(p50, 1)} / ${fmt(p90, 1)} s (${fmt(count)})` : "—";
}

function drawStats() {
  const rows = state.stats;
  const body = $("stats-body");
  body.innerHTML = "";
  const oldest = Date.now() - paintHistoryMs;
  const samples = [];
  for (const [id, sample] of paintSamples) {
    if (sample.at < oldest) paintSamples.delete(id);
    else if (sample.at >= state.statsStart) samples.push(sample);
  }
  for (const r of rows.sort((a, b) => b.located - a.located || b.heard - a.heard)) {
    const tr = document.createElement("tr");
    const values = samples.filter(s => s.profile === r.profile && s.band === r.band).map(s => s.seconds).sort((a, b) => a - b);
    const n = values.length;
    const median = n ? (values[Math.floor((n - 1) / 2)] + values[Math.floor(n / 2)]) / 2 : null;
    const p90 = n ? values[Math.ceil(0.9 * n) - 1] : null;
    tr.innerHTML = `<td>${esc(PROFILE[r.profile] || r.profile)} · ${esc(r.band)}</td><td>${r.heard}</td><td>${r.timed_two_plus}</td><td>${r.located}</td><td>${fmt(r.fixes_per_hour, 2)}</td><td>${km(r.median_error_km)} / ${km(r.p90_error_km)}</td><td>${pct(r.ellipse_68_coverage)} / ${pct(r.contour_coverage)}</td><td class="api-delay">${latencyText(r.median_publication_latency_s, r.p90_publication_latency_s, r.publication_latency_count)}</td><td class="paint-delay">${latencyText(median, p90, n)}</td>`;
    body.append(tr);
  }
  if (!rows.length) body.innerHTML = `<tr><td colspan="9" class="note">${state.mode === "live" ? "No live observation in this window." : "Live statistics only."}</td></tr>`;
}

function inspector(html) {
  const el = $("inspector");
  el.innerHTML = `<button class="icon-button close" id="inspector-close" aria-label="Close"><i data-lucide="x"></i></button>${html}`;
  el.hidden = false;
  $("inspector-close").onclick = () => { el.hidden = true; state.selected = null; receivers.clearLayers(); };
  icons();
}

async function selectTrack(key) {
  try {
    const d = await api(`/api/tracks/${encodeURIComponent(key)}`);
    const t = d.track, el = d.ellipse_95;
    state.selected = { track: key };
    receivers.clearLayers();
    const obs = d.observations.map((o) => `<div class="rx-row"><span class="rx-dot ${o.estimate ? "used" : "heard"}"></span><button class="link" data-id="${esc(o.id)}">${esc(utc(o.start_utc))}</button><span>${o.estimate ? `±${km(o.estimate.radius_km)}` : esc(o.status)}</span></div>`).join("");
    inspector(`<h3>${esc(trackName(t))}</h3><p class="sub">${esc(PROFILE[t.profile] || t.profile || "")} · ${esc(t.band || "")} · ${freq(t.frequency_hz)}</p>
      <dl><dt>Position</dt><dd>${positionText(t.center, gridDisplay(t.profile, t.callsign))}</dd>
      <dt>95% ellipse</dt><dd>${km(el.major_km)} × ${km(el.minor_km)}, axis ${fmt(el.azimuth_deg)}°</dd>
      <dt>Fused fixes</dt><dd>${t.fixes} (gate rejected ${t.rejected})</dd>
      <dt>Seen</dt><dd>${esc(utc(t.first_utc))} → ${esc(utc(t.last_utc))}</dd></dl>
      <p class="note">Covariance intersection of successive fixes: shared ionospheric and antenna errors are not assumed to average out.</p>
      <h4>Contributing observations</h4>${obs}`);
    for (const b of document.querySelectorAll("#inspector .link")) b.onclick = () => selectObservation(b.dataset.id);
    map.fitBounds(displayRegion(ellipse(t.center, el.major_km, el.minor_km, el.azimuth_deg), t.center, gridDisplay(t.profile, t.callsign)), { padding: [40, 40], maxZoom: 9 });
  } catch (e) { notify(e.message); }
}

async function selectObservation(id) {
  try {
    const e = await api(`/api/emissions/${encodeURIComponent(id)}`);
    state.selected = { observation: id };
    receivers.clearLayers();
    if ($("show-receivers").checked) {
      for (const r of e.receivers) {
        const cls = r.used ? "used" : r.confirmed ? "heard" : "";
        L.marker([r.position.lat, r.position.lon], { icon: L.divIcon({ className: "", html: `<div class="rx-icon ${cls}"></div>`, iconSize: [12, 12] }) })
          .bindTooltip(`${esc(r.label || r.id)}${r.used ? " · used" : r.confirmed ? " · heard" : r.reason ? " · " + esc(r.reason) : ""}`)
          .addTo(receivers);
      }
    }
    const status = e.estimate ? "located" : e.hypotheses?.length || e.detected ? "undetermined" : "none";
    const ests = e.estimate ? [e.estimate] : e.hypotheses || [];
    const ft8 = e.reference?.ft8;
    const rx = e.receivers.map((r) => `<div class="rx-row"><span class="rx-dot ${r.used ? "used" : r.confirmed ? "heard" : ""}"></span><span>${esc(r.label || r.id)}</span><span class="note">${r.used ? "used" : r.confirmed ? "timed" : esc(r.reason || "not confirmed")}</span></div>`).join("");
    const pairs = e.pairs.slice(0, 40).map((p) => `<div class="pair-row"><span>${esc(p.a.slice(0, 8))}→${esc(p.b.slice(0, 8))}</span><span>${us(p.delay_us)} ± ${us(p.sigma_us)}</span><span class="${p.accepted ? "good" : "bad"}">${p.accepted ? "accepted" : "rejected"}</span></div>`).join("");
    let evaluation = "";
    try {
      const ev = await api(`/api/emissions/${encodeURIComponent(id)}/evaluation`);
      evaluation = `<h4>Post-hoc check</h4><p class="note">Reference: ${esc(ev.reference_source)}${ev.grid ? " · locator " + esc(ev.grid) : ""}. Never used by the solver.</p>` +
        ev.solutions.map((s) => `<div class="pair-row"><span>solution</span><span>${km(s.error_km)}${s.error_range_km ? ` (${km(s.error_range_km[0])}–${km(s.error_range_km[1])})` : ""}</span><span>±${km(s.model_radius_km)}</span></div>`).join("");
    } catch { /* no hold-out reference for this observation */ }
    inspector(`<h3>${esc(e.identity?.callsign || e.title)}</h3><p class="sub">${esc(utc(e.start_utc))} · ${freq(e.frequency_hz)}</p>
      <p><span class="status-pill ${status}">${status === "located" ? "Located" : status === "undetermined" ? "Position undetermined" : "No detection"}</span></p>
      ${e.reason ? `<p class="note">${esc(e.reason)}</p>` : ""}
      ${ft8 ? `<dl><dt>Message</dt><dd>${esc(ft8.message)}</dd><dt>Heard by</dt><dd>${ft8.receivers.filter((r) => r.decoded).length} decoded · ${ft8.receivers.filter((r) => r.arrival_s != null).length} timed</dd></dl>` : ""}
      ${ests.map((s, i) => `<dl><dt>${ests.length > 1 ? "Hypothesis " + (i + 1) : "Estimate"}</dt><dd>${positionText(s.center, gridDisplay(e.reference?.profile, e.identity?.callsign))}</dd><dt>95% region</dt><dd>${km(s.radius_km)} max radius</dd><dt>Residual</dt><dd>${us(s.residual_us)} rms${s.scale ? ` · scale ${fmt(s.scale, 2)}` : ""}</dd></dl>`).join("")}
      <h4>Receivers</h4>${rx}${e.excluded?.length ? `<p class="note">${e.excluded.length} receiver(s) excluded by the RF verifier.</p>` : ""}
      <h4>Pair delays (B − A)</h4>${pairs || '<p class="note">No pair of timed receivers.</p>'}${evaluation}
      <p class="note">${esc(e.provenance?.note || "")}</p>`);
    const bounds = [...ests.flatMap((s) => displayRegion(polygon(s), s.center, gridDisplay(e.reference?.profile, e.identity?.callsign))), ...e.receivers.map((r) => [r.position.lat, r.position.lon])];
    if (bounds.length) map.fitBounds(bounds, { padding: [50, 50], maxZoom: 9 });
  } catch (err) { notify(err.message); }
}

async function waitJob(id) {
  for (let i = 0; i < 600; i++) {
    const j = await api(`/api/jobs/${encodeURIComponent(id)}`);
    if (j.status !== "running") return j;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("Job still running");
}

function setMode(mode) {
  resetPaintWindow();
  state.mode = mode;
  state.selected = null;
  state.scenario = "normal";
  $("inspector").hidden = true;
  receivers.clearLayers();
  $("mode-live").setAttribute("aria-selected", mode === "live");
  $("mode-replay").setAttribute("aria-selected", mode === "replay");
  $("mode-badge").textContent = mode === "live" ? "LIVE · OWN RF ONLY" : "REAL RECORDING · ARCHIVED IQ";
  $("mode-badge").className = `badge ${mode === "live" ? "live" : "replay"}`;
  $("replay-card").hidden = mode !== "replay";
  if (mode === "replay") setStatus("Archived recording", "ok");
  refresh();
}

function modal(title, html) {
  $("modal-title").textContent = title;
  $("modal-body").innerHTML = html;
  $("modal").showModal();
  icons();
}

$("mode-live").onclick = () => setMode("live");
$("mode-replay").onclick = () => setMode("replay");
for (const id of ["window", "band", "profile"]) $(id).onchange = refresh;
$("show-observations").onchange = () => drawFixes($("profile").value);
$("show-receivers").onchange = () => state.selected?.observation && selectObservation(state.selected.observation);
$("help").onclick = () =>
  modal("Method and limits", `<p>Every position comes from <b>our own IQ</b>: the same transmission recorded by several receivers whose clocks are GNSS-disciplined (KiwiSDR, GPSDO/PPS nodes) or synchronized from reference emitters. Differences in arrival time constrain the transmitter; nothing else (no spot feeds, no self-reported locations) is a solver input.</p>
  <ul><li><b>FT8</b>: decoded locally; each receiver is timed against the waveform rebuilt from the decoded message. The locator in a message is used only to measure accuracy afterwards.</li>
  <li><b>DCF77 and unknown emitters</b>: pairwise correlation of the received signal; occupied sub-bands are found by CFAR.</li>
  <li><b>Own nodes</b>: arrival times reported by edge nodes, clock-corrected from reference emitters.</li></ul>
  <p>A unique position needs at least four receivers around the transmitter. On HF a single fix is typically uncertain by hundreds of kilometres (ionospheric paths); LF ground wave reaches kilometres. Regions are 95% contours scaled by the data's own consistency.</p>
  <p><a href="https://github.com/s1lviu/meridian-tdoa/blob/main/docs/VALIDATION.md" target="_blank" rel="noopener noreferrer">Measured results and method</a></p>`);
$("network-open").onclick = async () => {
  try {
    const h = await api("/api/receivers/health");
    const now = Date.now();
    const blocked = h.receivers.filter((r) => r.blocked_until_ms > now);
    const rows = h.receivers.slice(0, 200).map((r) => `<tr><td>${esc(r.id)}</td><td>${r.ok}</td><td>${r.failed}</td><td>${fmt(r.seconds_24h / 60, 1)}</td><td>${r.blocked_until_ms > now ? "blocked" : "ok"}</td><td class="note">${esc(r.last_reason || "")}</td></tr>`).join("");
    modal("Receiver health", `<p>Fair use: ${fmt(h.policy.daily_minutes)} min per receiver per 24 h, ≥${h.policy.min_gap_s} s between connections. ${blocked.length} of ${h.receivers.length} receivers are currently blocked (refusals, failures).</p><div class="table-scroll"><table class="stats"><thead><tr><th>Receiver</th><th>OK</th><th>Failed</th><th>Min/24 h</th><th>State</th><th>Last reason</th></tr></thead><tbody>${rows}</tbody></table></div>`);
  } catch (e) { notify(e.message); }
};
$("capture-open").onclick = () => {
  const targets = (state.info?.targets || []).map((t) => `<option value="${esc(t.id)}">${esc(t.id)} · ${freq(t.frequency_hz)}</option>`).join("");
  modal("Request a capture", `<p>Captures use only receivers whose operators publish external access, within the fair-use budget.</p>
    <label>Target<select id="capture-target">${targets}</select></label>
    <label>Receiver IDs (optional, comma separated, 3–12)<input type="text" id="capture-sources" /></label>
    <label>Administrator token<input type="password" id="capture-token" autocomplete="off" /></label>
    <button id="capture-go" class="primary"><i data-lucide="radio"></i>Start</button>`);
  $("capture-go").onclick = async () => {
    const ids = $("capture-sources").value.split(",").map((s) => s.trim()).filter(Boolean);
    try {
      const job = await api("/api/capture", { method: "POST", headers: { "Content-Type": "application/json", Authorization: "Bearer " + $("capture-token").value }, body: JSON.stringify({ target_id: $("capture-target").value, source_ids: ids.length ? ids : null }) });
      $("capture-token").value = "";
      $("modal").close();
      notify("Capture started");
      const done = await waitJob(job.job_id);
      notify(`Capture ${done.status}${done.error ? ": " + done.error : ""}`);
      refresh();
    } catch (e) { notify(e.message); }
  };
};
async function replay(fault) {
  const d = state.info?.datasets?.[0];
  if (!d) return notify("No recording bundled");
  try {
    const job = await api("/api/replay", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ dataset_id: d.id, fault_receiver: fault ? d.receivers[d.receivers.length - 1].id : null }) });
    if (!job.cached) await waitJob(job.job_id);
    state.scenario = fault ? "sync-loss" : "normal";
    state.selected = null;
    refresh();
  } catch (e) { notify(e.message); }
}
$("replay-open").onclick = () => replay(false);
$("sync-loss").onclick = () => replay(true);

function stream() {
  if (!window.EventSource) return;
  const es = new EventSource("/api/stream");
  es.onopen = () => { state.stream = "open"; };
  es.onerror = () => { resetPaintWindow(); state.stream = "reconnecting"; if (state.mode === "live") setStatus("Live updates reconnecting…", "warn"); };
  let timer;
  es.addEventListener("change", () => {
    clearTimeout(timer);
    timer = setTimeout(() => { if (state.mode === "live" && !document.hidden) refresh(); }, 800);
  });
}

async function boot() {
  icons();
  try {
    state.info = await api("/api/info");
    const d = state.info.datasets?.[0];
    if (d) $("replay-text").textContent = `${d.title} · ${utc(d.capture_utc)} · ${d.receivers.length} receivers. Archived IQ, not live.`;
    const params = new URLSearchParams(location.search);
    stream();
    setMode(params.has("replay") || !state.info.live_enabled ? "replay" : "live");
    if (params.get("track")) selectTrack(params.get("track"));
    if (params.get("observation")) selectObservation(params.get("observation"));
  } catch (e) {
    setStatus("API unavailable", "warn");
    notify(e.message);
  }
}
// Fallback refresh when push updates are unavailable; receivers are never contacted from here.
setInterval(() => { if (state.mode === "live" && !document.hidden && state.stream !== "open") refresh(); }, 30000);
boot();
