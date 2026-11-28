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

const state = { info: null, data: null, tracks: [], stats: [], statsStart: 0, selected: null, refreshing: false, pending: false };
const map = L.map("map", { worldCopyJump: true, zoomSnap: 0.25, minZoom: 0, maxZoom: 14, zoomControl: false, zoomAnimation: !reducedMotion() });
const openingZoom = Math.max(0, Math.min(2.5, Math.floor(4 * Math.log2(map.getSize().x / 256)) / 4));
map.setView([22, 0], openingZoom);
L.control.zoom({ position: "bottomright" }).addTo(map);
L.control.scale({ imperial: false, position: "bottomright" }).addTo(map);
const land = L.layerGroup().addTo(map);
const cities = L.layerGroup().addTo(map);
const fixes = L.layerGroup().addTo(map);
const tracksLayer = L.layerGroup().addTo(map);
const receivers = L.layerGroup().addTo(map);
const selectedEvidence = L.layerGroup().addTo(map);

let places = [];
initializeMapView();
let worldRequest;
function loadWorld() {
  worldRequest ??= fetch("/world.geojson").then(r => {
    if (!r.ok) throw Error("Basemap unavailable");
    return r.json();
  }).then(g => { worldGeometry = g; drawLand(); return g; }).catch(err => { worldRequest = null; throw err; });
  return worldRequest;
}
loadWorld().catch(() => {});
fetch("/places.json").then((r) => (r.ok ? r.json() : [])).then((p) => { places = p; drawCities(); placeLabels(); }).catch(() => {});
function drawCities() {
  cities.clearLayers();
  const z = map.getZoom();
  const bounds = map.getBounds().pad(0.05);
  for (const [minZoom, name, lat, lon] of places) {
    if (z < minZoom + 1.5) continue;
    const point = visiblePoint([lat, lon]);
    if (bounds.contains(point)) L.marker(point, { interactive: false, keyboard: false, icon: L.divIcon({ className: "city", html: esc(name), iconSize: null }) }).addTo(cities);
  }
}

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

// Countdown to the scheduler's next capture window, from the small /api/monitor snapshot.
// When a window starts the scheduler announces the following one, so the page asks again;
// receivers are never contacted from here.
const monitor = { configured: false, nextAt: 0, targets: [], notice: null, skew: 0, polledAt: 0, polls: 0 };
async function loadMonitor() {
  monitor.polledAt = Date.now();
  try {
    const r = await fetch("/api/monitor");
    if (r.ok) {
      // HTTP dates have one-second resolution: correct only a clearly wrong local clock.
      const server = Date.parse(r.headers.get("date") || "");
      const skew = isFinite(server) ? server - Date.now() : 0;
      monitor.skew = Math.abs(skew) > 2000 ? skew : 0;
      const o = await r.json();
      const nextAt = o?.next_at ? Date.parse(o.next_at) : 0;
      if (nextAt > Date.now() + monitor.skew) monitor.polls = 0;
      Object.assign(monitor, { configured: !!o, nextAt, targets: o?.next_target_ids || [], notice: o?.notice || null });
    }
  } catch { /* keep the last known schedule */ }
  tick();
}
const clock = (ms) => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};
function tick() {
  const el = $("next-window");
  if (!monitor.configured || (!monitor.nextAt && !monitor.notice)) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  if (!monitor.nextAt) {
    el.className = "next-window paused";
    el.textContent = el.title = monitor.notice;
    return;
  }
  const left = monitor.nextAt - (Date.now() + monitor.skew);
  const targets = monitor.targets.length ? `<span class="targets"> · ${esc(monitor.targets.join(", "))}</span>` : "";
  el.className = "next-window";
  el.title = `Scheduled for ${utc(monitor.nextAt)}${monitor.notice ? " · " + monitor.notice : ""}`;
  if (left > 0) {
    el.innerHTML = `Next window in <b>${clock(left)}</b>${targets}`;
  } else {
    el.innerHTML = `Window starting…${targets}`;
    if (Date.now() - monitor.polledAt > 2000 && monitor.polls < 30) {
      monitor.polls++;
      loadMonitor();
    }
  }
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
  if (!grid) return points.map(([lat, lon]) => [lat, nearLongitude(lon, center.lon)]);
  const cell = locatorCell(center);
  const lats = points.map(([lat]) => lat).concat(cell.south, cell.south + 1);
  const lons = points.map(([, lon]) => center.lon + ((lon - center.lon + 540) % 360) - 180);
  const cellWest = nearLongitude(cell.west + 1, center.lon) - 1;
  const west = Math.min(cellWest, 2 * Math.floor(Math.min(...lons) / 2));
  const east = Math.max(cellWest + 2, 2 * Math.ceil(Math.max(...lons) / 2));
  const south = Math.floor(Math.min(...lats)), north = Math.ceil(Math.max(...lats));
  return [[south, west], [north, west], [north, east], [south, east]];
}

function interval() {
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
  if (!document.hidden) refresh();
});
function measureMap(scope) {
  const current = new Set(), arrived = [];
  for (const {track} of state.tracks) {
    const id = track.observations.at(-1);
    const at = Date.parse(track.last_utc);
    current.add(id);
    if (scope !== viewScope || state.selected || !overview.drawnKeys?.has(track.key) || !track.rf_utc || at < visibleSince || lastDrawn.has(id)) continue;
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
    const scope = [band, profile, $("window").value].join("/");
    const q = `mode=live&compact=true&start=${start}&end=${end}&band=${band}`;
    loadMonitor();
    const [data, tracks, stats] = await Promise.all([
      api(`/api/emissions?${q}`),
      api(`/api/tracks?since=${start}&band=${band}`),
      api(`/api/stats?start=${start}&end=${end}`),
    ]);
    if (scope !== [$("band").value, $("profile").value, $("window").value].join("/")) { state.pending = true; return; }
    state.data = data;
    state.tracks = (tracks?.tracks || []).filter((t) => profile === "all" || t.track.profile === profile || (profile === "dcf77_prn_v1" && t.track.key === "ref:DCF77"));
    recordArrivals(data, scope, profile);
    if (!state.selected) { drawLists(profile); updateOverview(); }
    state.stats = stats?.rows || [];
    state.statsStart = Date.parse(start);
    drawStats();
    measureMap(scope);
    const obs = data.monitoring?.segments?.length || 0;
    const capture = state.info?.live_enabled === false ? " · capture off on this server" : "";
    setStatus(`${obs} ${obs === 1 ? "observation" : "observations"} in window · ${state.tracks.length} located${capture}`, state.stream === "open" ? "ok" : "warn");
  } catch (e) {
    setStatus("API unavailable", "warn");
    notify(e.message);
  } finally {
    state.refreshing = false;
    if (state.pending) { state.pending = false; refresh(); }
  }
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
  if (!rows.length) body.innerHTML = `<tr><td colspan="9" class="note">No live observation in this window.</td></tr>`;
}

async function waitJob(id) {
  for (let i = 0; i < 600; i++) {
    const j = await api(`/api/jobs/${encodeURIComponent(id)}`);
    if (j.status !== "running") return j;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("Job still running");
}

function modal(title, html) {
  $("modal-title").textContent = title;
  $("modal-body").innerHTML = html;
  $("modal").showModal();
  icons();
}

for (const id of ["window", "band", "profile"]) $(id).onchange = () => { resultLimits.clear(); refresh(); };
$("search").oninput = () => { resultLimits.clear(); drawLists($("profile").value); updateOverview(); };
$("show-observations").onchange = drawSingles;
$("show-receivers").onchange = () => { drawSelectionLayers(); placeLabels(); };
$("map").addEventListener("keydown", e => { if (e.key === "Escape" && state.selected) closeInspection(); });
$("inspector").addEventListener("keydown", e => { if (e.key === "Escape") closeInspection(); });
$("new-observations").onclick = closeInspection;
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
function stream() {
  if (!window.EventSource) return;
  const es = new EventSource("/api/stream");
  es.onopen = () => { state.stream = "open"; };
  es.onerror = () => { resetPaintWindow(); state.stream = "reconnecting"; setStatus("Live updates reconnecting…", "warn"); };
  let timer;
  es.addEventListener("change", () => {
    clearTimeout(timer);
    timer = setTimeout(() => { if (!document.hidden) refresh(); }, 800);
  });
}

// The map shows live data only: observations from our own captures and nodes, never archives.
async function boot() {
  icons();
  try {
    state.info = await api("/api/info");
    const params = new URLSearchParams(location.search);
    stream();
    resetPaintWindow();
    tick();
    refresh();
    if (params.get("track")) selectTrack(params.get("track"));
    if (params.get("observation")) selectObservation(params.get("observation"));
  } catch (e) {
    setStatus("API unavailable", "warn");
    notify(e.message);
  }
}
// Fallback refresh when push updates are unavailable; receivers are never contacted from here.
setInterval(() => { if (!document.hidden && state.stream !== "open") refresh(); }, 30000);
setInterval(tick, 1000);
boot();
