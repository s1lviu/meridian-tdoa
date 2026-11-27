"use strict";
const $ = (id) => document.getElementById(id);
const icons = () => window.lucide?.createIcons();
const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const link = (url, text) =>
  `<a href="${esc(/^https?:\/\//.test(url) ? url : "#")}" target="_blank" rel="noopener noreferrer">${esc(text)}</a>`;
const fmt = (n, d = 1) =>
  Number(n).toLocaleString("en-GB", {
    minimumFractionDigits: d,
    maximumFractionDigits: d,
  });
const frequency = (f) => f >= 1e6 ? `${fmt(f / 1e6, 3)} MHz` : `${fmt(f / 1000, f % 1000 ? 2 : 0)} kHz`;
const utc = (t) => new Date(t).toISOString().slice(11, 23);
const utcDate = (t) =>
  new Date(t).toLocaleDateString("en-GB", {
    timeZone: "UTC",
    day: "2-digit",
    month: "short",
    year: "numeric",
  });
const referenceProfile = (e) => e?.reference?.profile || e?.profile;
const isDcf = (e) => referenceProfile(e) === "dcf77_prn_v1";
const isIbp = (e) => referenceProfile(e) === "ncdxf_cw_v1";
const verified = (e) => isDcf(e) || isIbp(e);
const signalKey = e => e ? JSON.stringify([referenceProfile(e) || null, e.identity?.callsign || (isDcf(e) ? "DCF77" : e.id), e.frequency_hz]) : null;
const worldwide = () => state.info?.targets?.some(t => t.verification === "ncdxf_cw_v1");
const name = (e) => e.identity?.callsign || (isDcf(e) ? "DCF77 time signal" : "unknown callsign");
const usedIds = (e) => e.used_ids ?? e.receivers.filter((r) => r.used).map((r) => r.id);
const receptionOn = () => $("show-reception").checked;
const positionObservation = e => (e.estimate || e.hypotheses?.length) ? e : e.last_position;
const state = {
  mode: "replay",
  scenario: "normal",
  info: null,
  selected: null,
  data: null,
  request: 0,
  busy: false,
  manualClose: false,
  datasetId: null,
  selectionRequest: 0,
  pinned: false,
  compareThree: false,
  regionKey: null,
  historyKey: null,
  fitFirst: false,
  beacons: null,
  clockOffset: 0,
  overview: false,
  requestedStation: null,
  followingBeacon: null,
};
const map = L.map("map", {
  zoomControl: true,
  minZoom: 0,
  zoomSnap: 0.25,
  maxZoom: 14,
  worldCopyJump: true,
}).setView([49.5, 6.5], 5);
const digital = digitalActivity(map);
L.control
  .scale({ imperial: false, position: "bottomleft", maxWidth: 80 })
  .addTo(map);
map.createPane("fallback");
map.getPane("fallback").style.zIndex = "100";
map.createPane("countryLabels");
map.getPane("countryLabels").style.zIndex = "210";
map.getPane("countryLabels").style.pointerEvents = "none";
const countryLabels = L.layerGroup().addTo(map);
let countries = [];
function drawCountries() {
  countryLabels.clearLayers();
  const occupied = [], bounds = map.getBounds(), zoom = map.getZoom();
  for (const p of countries) {
    if (p.MIN_LABEL > zoom || !bounds.contains([p.LABEL_Y, p.LABEL_X])) continue;
    const label = p.NAME_EN || p.NAME, width = 8 + label.length * 6.5;
    const pos = map.latLngToContainerPoint([p.LABEL_Y, p.LABEL_X]);
    const box = L.bounds([pos.x - width/2, pos.y-8], [pos.x+width/2,pos.y+8]);
    if (occupied.some(b => b.intersects(box))) continue;
    occupied.push(box);
    L.marker([p.LABEL_Y,p.LABEL_X], {pane:"countryLabels", interactive:false, keyboard:false,
      icon:L.divIcon({html:esc(label),className:"country-label",iconSize:[width,16],iconAnchor:[width/2,8]})}).addTo(countryLabels);
  }
}
map.on("moveend", drawCountries);
fetch("/world.geojson")
  .then((r) => (r.ok ? r.json() : null))
  .then((data) => {
    if (data) {
      L.geoJSON(data, {
        pane: "fallback",
        interactive: false,
        style: {
          color: "#425463",
          weight: 0.6,
          fillColor: "#263640",
          fillOpacity: 1,
        },
      }).addTo(map);
      countries = data.features.map(f => f.properties).filter(p => Number.isFinite(p.LABEL_Y) && Number.isFinite(p.LABEL_X))
        .sort((a,b) => a.LABELRANK - b.LABELRANK);
      drawCountries();
    }
  })
  .catch(() => {});
map.attributionControl.addAttribution('<a href="https://www.naturalearthdata.com/">Natural Earth</a> · local vector basemap');
const cityLabels = L.layerGroup().addTo(map);
let places = [];
function drawPlaces() {
  cityLabels.clearLayers();
  const zoom = map.getZoom();
  if (zoom < 6) return;
  const bounds = map.getBounds(), middle = map.getCenter().lng, occupied = [];
  for (const [minZoom, city, lat, lon] of places) {
    if (minZoom > zoom) break;
    const point = L.latLng(lat, lon + 360 * Math.round((middle - lon) / 360));
    if (!bounds.contains(point)) continue;
    const p = map.latLngToContainerPoint(point), width = 10 + city.length * 6;
    const box = L.bounds([p.x - 3, p.y - 9], [p.x + width, p.y + 9]);
    if (occupied.some((other) => other.intersects(box))) continue;
    occupied.push(box);
    L.marker(point, {
      pane: "countryLabels", interactive: false, keyboard: false,
      icon: L.divIcon({ className: "city-label", html: esc(city), iconSize: [width, 16], iconAnchor: [2, 8] }),
    }).addTo(cityLabels);
  }
}
fetch("/places.json").then((r) => r.ok ? r.json() : []).then((data) => {
  places = data;
  drawPlaces();
}).catch(() => {});
map.on("moveend", drawPlaces);
const regions = L.layerGroup().addTo(map),
  network = L.layerGroup().addTo(map);
map.createPane("receiverPool").style.zIndex = "310";
const pool = L.layerGroup().addTo(map), directoryOnly = L.layerGroup().addTo(map);
const poolRenderer = L.svg({ pane: "receiverPool" });
const directoryRadius = () => map.getZoom() < 4 ? 2.5 : 4.5;
map.on("zoomend", () => directoryOnly.eachLayer(marker => marker.setRadius(directoryRadius())));
const bandName = e => state.beacons?.channels.find(c => c.frequency_hz === e?.frequency_hz)?.band || (isDcf(e) ? "LF" : frequency(e.frequency_hz));
const matchesPoolBand = (r, band) => band === "all" || band === "hf" || r.bands.includes(band);
const showsHfEligibility = band => band === "all" || band === "hf" || state.beacons.channels.some(c => c.band === band);

function drawDirectoryCount() {
  if (state.mode !== "live" || !state.beacons?.directory_utc) return;
  const [west, south, east, north] = boundsQuery(), band = $("band").value;
  const inView = ({position: [lat, lon]}) => lat >= south && lat <= north &&
    (west <= east ? lon >= west && lon <= east : lon >= west || lon <= east);
  const candidates = state.beacons.receiver_pool.filter(inView);
  const listed = candidates.length + state.beacons.other_receivers.filter(inView).length;
  const eligibility = showsHfEligibility(band)
    ? `${candidates.filter(r => matchesPoolBand(r, band)).length} eligible for ${band === "all" || band === "hf" ? "HF beacon channels" : band}`
    : "Eligibility shown on the five HF beacon channels";
  $("directory-view").textContent = `Directory in this view · ${listed} listed · ${eligibility}`;
}
function addDirectoryMarker(r, eligible, hf) {
  const status = eligible ? "Eligible HF receiver" : "Directory receiver";
  const reason = r.reason || (eligible || !hf
    ? `Eligible beacon bands · ${r.bands.join(", ")}`
    : "Selected channel outside the published tuning range");
  const marker = L.circleMarker(r.position, { renderer: poolRenderer, receiverId: r.id,
    radius: eligible ? 3.8 : directoryRadius(), weight: eligible ? 1.3 : 1, color: eligible ? "#98c7dc" : "#9ca9b3",
    fillColor: "#5c9bb2", fillOpacity: eligible ? .7 : .08 });
  marker.bindTooltip(`${esc(r.label)} · ${status}`).bindPopup(() => {
    const stamp = state.beacons.directory_utc;
    const gnss = r.fixes_min == null ? "" : `<br>GNSS · ${r.gps_good} usable satellites · ${r.fixes_min} fixes/min`;
    const channels = r.users_max ? `<br>Channels in use · ${r.users}/${r.users_max}` : "";
    return `<strong>${esc(r.label)}</strong><p>${status}<br>${esc(reason)}${gnss}${channels}</p><small>Directory update · ${utcDate(stamp)} ${utc(stamp).slice(0, 5)} UTC</small>`;
  }, {className: "receiver-popup", maxWidth: 270}).addTo(eligible ? pool : directoryOnly);
}
function drawPool() {
  pool.clearLayers();
  directoryOnly.clearLayers();
  $("pool-legend").hidden = true;
  $("directory-legend").hidden = true;
  if (!state.beacons || state.mode !== "live" || !worldwide() || !receptionOn()) return;
  if (!state.beacons.directory_utc) {
    $("directory-view").textContent = "Waiting for the public receiver directory";
    return;
  }
  const band = $("band").value, hf = showsHfEligibility(band);
  const candidates = state.beacons.receiver_pool;
  $("show-pool").disabled = !hf;
  if ($("show-directory").checked) {
    for (const r of state.beacons.other_receivers) addDirectoryMarker(r, false, hf);
    for (const r of candidates.filter(r => !matchesPoolBand(r, band))) addDirectoryMarker(r, false, hf);
    $("directory-legend").hidden = !directoryOnly.getLayers().length;
  }
  if ($("show-pool").checked) {
    for (const r of candidates.filter(r => matchesPoolBand(r, band))) addDirectoryMarker(r, true, hf);
    $("pool-legend").hidden = !pool.getLayers().length;
  }
  drawDirectoryCount();
}
const nowUtc = () => Date.now() + state.clockOffset;
function drawSchedule() {
  if (state.mode !== "live" || !state.beacons) return;
  const slot = Math.floor(nowUtc() / 10000);
  state.beacons.channels.forEach((c, i) => {
    const btn = document.querySelector(`[data-band="${c.band}"]`);
    if (!btn) return;
    btn.setAttribute("aria-pressed", String($("band").value === c.band));
    const calls = state.beacons.callsigns;
    const call = calls[((slot - i) % calls.length + calls.length) % calls.length];
    if (btn.querySelector("strong").textContent !== call) btn.querySelector("strong").textContent = call;
  });
}
async function loadBeacons() {
  if (!worldwide()) return;
  try {
    const began = Date.now();
    const data = await api("/api/beacons");
    data.other_receivers ??= [];
    state.clockOffset = Date.parse(data.server_utc) - (began + Date.now()) / 2;
    state.beacons = data;
    $("band-schedule").innerHTML = data.channels.map(c => `<button class="band-channel" data-band="${esc(c.band)}" type="button"><span>${esc(c.band)} · Scheduled</span><strong>—</strong><small>${frequency(c.frequency_hz)}</small></button>`).join("");
    for (const b of $("band-schedule").querySelectorAll("button")) b.onclick = () => chooseBand(b.dataset.band);
    drawSchedule();
    drawPool();
    drawRoster();
  } catch {
    if (!state.beacons) $("directory-view").textContent = "Receiver directory and schedule unavailable";
  }
}
function drawBandStats(rows) {
  if (state.mode !== "live" || !worldwide()) return;
  const km = n => n == null ? "—" : fmt(n) + " km";
  $("band-stats").innerHTML = (rows || []).map(r => `<tr data-stats-band="${esc(r.band)}" class="${$("band").value === r.band.toLowerCase() ? "band-selected" : ""}"><th scope="row">${esc(r.band)}</th><td>${r.scheduled_attempts}</td><td>${r.sampled_slots}</td><td>${r.verified_slots}</td><td>${r.distinct_beacons}</td><td>${r.unique_fixes}</td><td>${r.ambiguous}</td><td>${r.evaluated}</td><td>${km(r.median_error_km)}</td><td>${km(r.p90_error_km)}</td></tr>`).join("");
}
function drawRoster() {
  if (!state.beacons || !state.data || state.mode !== "live") return;
  const heard = new Map();
  for (const e of (state.data.signals || []).filter(isIbp)) {
    const call = e.identity.callsign;
    if (!heard.has(call)) heard.set(call, []);
    heard.get(call).push(e);
  }
  $("roster-count").textContent = `${heard.size} of ${state.beacons.callsigns.length} verified · selected band and UTC interval`;
  const showActivity = receptionOn() && showsHfEligibility($("band").value);
  $("map-activity").hidden = !showActivity;
  $("map-method").hidden = showActivity;
  const located = [...heard.values()].filter(group => group.some(e => e.estimate || e.last_position?.estimate)).length;
  $("map-activity").textContent = `HF beacons · ${heard.size} heard · ${located} located ▾`;
  const calls = [...heard.keys(), ...state.beacons.callsigns.filter(call => !heard.has(call))];
  $("roster-list").innerHTML = calls.map(call => {
    const links = (heard.get(call) || []).map(e => {
      const latest = `<button type="button" data-heard="${esc(e.id)}" title="${esc(utc(e.start_utc))} UTC">${esc(bandName(e))} · ${e.confirmed_count} RX · ${e.estimate ? "TDoA fix" : e.hypotheses.length ? "ambiguous" : "heard only"}</button>`;
      const prior = e.last_position;
      return latest + (prior ? `<button type="button" data-heard="${esc(prior.id)}">Last TDoA · ${utc(prior.start_utc).slice(0,8)} UTC</button>` : "");
    }).join("");
    return `<div class="roster-item"><strong>${esc(call)}</strong>${links || "<small>No verified reception</small>"}</div>`;
  }).join("");
}
$("map-activity").onclick = () => {
  $("beacon-roster").open = true;
  $("beacon-roster").querySelector("summary").focus();
  $("beacon-roster").scrollIntoView({block:"center"});
};
$("roster-list").onclick = async event => {
  const id = event.target.closest("[data-heard]")?.dataset.heard;
  if (id) {
    await inspect(id);
    if (state.selected?.id === id) fitNetwork();
    $("map").scrollIntoView({ block: "center" });
  }
};
let refreshTimer;
map.on("moveend", () => {
  const c = map.getCenter();
  drawDirectoryCount();
  $("map-position").textContent =
    `${Math.abs(c.lat).toFixed(2)}°${c.lat >= 0 ? "N" : "S"} / ${Math.abs(c.lng).toFixed(2)}°${c.lng >= 0 ? "E" : "W"} / ${map.getZoom()}×`;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => refresh(false), 180);
  if (state.selected) renderInspector(selectedView());
});
async function api(path, options) {
  const response = await fetch(path, options);
  let data;
  try {
    data = await response.json();
  } catch {
    throw new Error(`Invalid API response (${response.status})`);
  }
  if (!response.ok) throw new Error(data.error || `Error ${response.status}`);
  return data;
}
function notify(text) {
  $("toast").textContent = text;
  $("toast").hidden = false;
  clearTimeout(notify.timer);
  notify.timer = setTimeout(() => ($("toast").hidden = true), 5000);
}
function boundsQuery() {
  const b = map.getBounds();
  const wrap = (x) => ((((x + 180) % 360) + 360) % 360) - 180;
  return b.getEast() - b.getWest() >= 360
    ? [-180, Math.max(-90, b.getSouth()), 180, Math.min(90, b.getNorth())]
    : [
        wrap(b.getWest()),
        Math.max(-90, b.getSouth()),
        wrap(b.getEast()),
        Math.min(90, b.getNorth()),
      ];
}
function query() {
  const start = new Date($("start").value + "Z"),
    end = new Date($("end").value + "Z");
  if (!Number.isFinite(+start) || !Number.isFinite(+end) || end <= start)
    throw new Error("Choose a valid UTC interval with the end after the start.");
  return new URLSearchParams({
    bbox: boundsQuery().join(","),
    band: $("band").value,
    start: start.toISOString(),
    end: end.toISOString(),
    mode: state.mode,
    scenario: state.scenario,
    compact: String(state.mode === "live"),
  });
}
async function refresh(selectFirst = false) {
  if (!state.info) return;
  const serial = ++state.request;
  try {
    const q = query();
    $("filter-error").textContent = "";
    const data = await api("/api/emissions?" + q);
    if (serial !== state.request) return;
    state.data = data;
    $("api-status").textContent = "API connected · Rust";
    $("api-dot").style.background = "var(--green)";
    renderList(data);
    renderCoverage(data.monitoring);
    renderMonitor();
    drawBandStats(data.band_stats);
    drawRoster();
    const available = observations();
    const follow = state.mode === "live" && $("follow-live").checked && !state.pinned;
    if (state.requestedStation && state.mode === "live") {
      const wanted = state.requestedStation;
      const matches = data.signals.filter(e => (isDcf(e) ? "DCF77" : e.identity?.callsign)?.toUpperCase() === wanted);
      const signal = matches.find(e => positionObservation(e)?.estimate) || matches.find(positionObservation) || matches[0];
      if (signal) {
        const row = positionObservation(signal) || signal;
        if (!row.estimate) $("unresolved-stations").open = true;
        await inspect(row.id, true, true);
      } else {
        $("station-notice").textContent = `Waiting for a confirmed reception of ${wanted} on the selected band and UTC interval.`;
        $("station-notice").hidden = false;
      }
    } else if (follow && state.followingBeacon) {
      const next = data.signals.find(s => signalKey(s) === state.followingBeacon);
      if (next && next.id !== state.selected?.id) await selectEmission(next.id);
      else if (!next && state.selected && Date.parse(state.selected.end_utc) <= Date.parse(q.get("start"))) clearSelection();
    } else if ((selectFirst || !state.selected || follow) && !state.manualClose) {
      const first = state.mode === "live" ? available.find(verified) || available[0] : available[0];
      if (first && first.id !== state.selected?.id) await selectEmission(first.id);
      else if (!first && (selectFirst || (follow && state.selected &&
        Date.parse(state.selected.end_utc) <= Date.parse(q.get("start"))))) clearSelection();
    }
    if (serial !== state.request) return;
    renderRegions(data.emissions);
    renderHistory();
    $("map-note").hidden = true;
    $("empty-map").hidden = !!state.selected || available.length > 0;
    if (!state.selected) {
      drawSpectrum([]);
      const h = $("empty-map").querySelector("h2"),
        p = $("empty-map").querySelector("p");
      if (data.monitoring.segments.some((s) => s.status === "no_verified_beacon")) {
        h.textContent = "Channel sampled";
        p.textContent = "No scheduled callsign was verified in these IQ samples.";
      } else if (data.monitoring.segments.some((s) => s.status === "no_detection")) {
        h.textContent = "No emission detected";
        p.textContent =
          "The detector found no emission in the recorded channels. Gaps remain unmonitored.";
      } else if (data.total_matching > 0) {
        h.textContent = "No region in this view";
        p.textContent =
          "Results exist outside the visible map. Pan or zoom to see them.";
      } else {
        h.textContent = "Unmonitored interval";
        p.textContent = state.mode === "live"
          ? "No processed receiver window matches this band and UTC interval."
          : "No recordings match these filters. Missing data does not mean there were no emissions.";
      }
      if (state.mode === "live" && (worldwide() || state.info.digital_reports) && !available.length) {
        $("map-note").textContent = (state.info.digital_reports ? "TDoA · " : "") + h.textContent + " · " + p.textContent;
        $("map-note").hidden = false;
        $("empty-map").hidden = true;
      }
    }
    icons();
  } catch (err) {
    if (serial !== state.request) return;
    $("filter-error").textContent = err.message;
    $("api-status").textContent = "Data unavailable";
    $("api-dot").style.background = "var(--amber)";
    if (state.mode === "live") {
      $("monitor-clock").hidden = true;
      $("observation-title").textContent = "Connection lost · waiting for the next update";
    }
  }
}
function renderList(data) {
  const latestIds = state.mode === "live" && data.signals ? new Set(data.signals.flatMap(e => [e.id, e.last_position?.id])) : null;
  const recent = e => !latestIds || !verified(e) || latestIds.has(e.id);
  const visible = data.emissions.filter(recent), unlocated = data.unlocated.filter(recent);
  $("emission-count").textContent = visible.length;
  const card = (e, unknown = false) =>
    `<button class="emission-card ${unknown ? "unlocated-card" : ""} ${state.selected?.id === e.id ? "selected" : ""}" data-emission="${esc(e.id)}"><div class="emission-card-top"><strong>${esc(name(e))}</strong><span class="micro-tag">${unknown ? "UNDETERMINED" : e.hypotheses?.length ? "AMBIGUOUS" : "TDoA"}</span></div><div class="emission-frequency">${frequency(e.frequency_hz)}</div><div class="emission-meta"><i data-lucide="clock-3"></i>${utc(e.start_utc).slice(0, 8)} UTC <span>·</span> ${e.confirmed_count ?? e.receivers.filter((r) => r.confirmed).length} RX</div></button>`;
  $("emissions").innerHTML =
    visible.map((e) => card(e)).join("") ||
    '<div class="empty-list">No estimated region in the current view.</div>';
  $("unlocated").innerHTML = unlocated.length
    ? `<div class="unlocated-title">Heard · position undetermined</div>${unlocated.map((e) => card(e, true)).join("")}`
    : "";
  document.querySelectorAll("[data-emission]").forEach(
    (b) =>
      (b.onclick = () => {
        inspect(b.dataset.emission, true);
      }),
  );
  if (state.mode === "live") renderTransmitters(data.signals || []);
}
function renderTransmitters(signals) {
  const located = [], unresolved = [];
  for (const signal of signals) {
    const row = positionObservation(signal) || signal;
    const rx = row.confirmed_count ?? 0;
    const time = utc(row.start_utc).slice(0, 8) + " UTC";
    const band = isDcf(row) ? "LF" : bandName(row);
    let detail = row.estimate ? `${usedIds(row).length} RX · ${fmt(row.estimate.radius_km)} km radius` :
      row.hypotheses?.length ? `${row.hypotheses.length} possible regions` :
      `${rx} RX` + (row.pair_count ? ` · ${row.accepted_pair_count}/${row.pair_count} accepted delay pairs` : "");
    if (row.id !== signal.id) detail += ` · heard again ${utc(signal.start_utc).slice(0,8)} UTC`;
    const entry = `<button type="button" data-station-observation="${esc(row.id)}"><strong>${esc(isDcf(row) ? "DCF77" : name(row))}</strong><span>${esc(band)} · ${time} · ${detail}</span></button>`;
    (row.estimate ? located : unresolved).push(entry);
  }
  const panel = $("unresolved-stations");
  panel.hidden = !unresolved.length;
  panel.querySelector("summary").textContent = `Heard without a unique position (${unresolved.length})`;
  const lists = [[$("located-stations"), located.join("") || "<p>No unique TDoA position in this interval on the selected band.</p>"],
    [panel.querySelector(".station-list"), unresolved.join("")]];
  for (const [list, html] of lists) if (list.innerHTML !== html) list.innerHTML = html;
}
$("transmitters").onclick = event => {
  const id = event.target.closest("[data-station-observation]")?.dataset.stationObservation;
  if (id) inspect(id, true, true);
};
function observations() {
  if (!state.data) return [];
  const { history = [], signals = [], emissions, unlocated } = state.data;
  const byId = new Map([...emissions, ...unlocated, ...history, ...signals].map((e) => [e.id, e]));
  return [...byId.values()].sort((a, b) => Date.parse(b.start_utc) - Date.parse(a.start_utc) || a.id.localeCompare(b.id));
}
function previousReference(e) {
  if (!verified(e)) return null;
  return observations().find((item) => signalKey(item) === signalKey(e) &&
    Date.parse(item.start_utc) < Date.parse(e.start_utc));
}
function selectedView() {
  const e = state.selected;
  const three = e?.reference?.three_receivers;
  if (!state.compareThree || !three) return e;
  return { ...e, estimate: three.solutions.length === 1 ? three.solutions[0] : null,
    hypotheses: three.solutions.length > 1 ? three.solutions : [],
    reason: three.reason || "These three receivers do not determine a position.",
    receivers: e.receivers.map((r) => ({ ...r, used: three.receiver_ids.includes(r.id) })) };
}
function renderMonitor() {
  if (state.mode !== "live") return;
  const m = state.data?.observatory;
  $("replay").hidden = !!m;
  $("monitor-clock").hidden = !m?.next_at;
  if (!m) {
    $("observation-title").textContent = state.info.live_enabled ? "Approved network · capture on demand" : "Live capture is not configured";
    return;
  }
  const status = m.last_job?.status;
  const messages = {
    running: "Capturing and checking synchronized IQ",
    completed: "Last window processed",
    awaiting_review: "Last window awaits signal verification",
    failed: "Last attempt failed",
    interrupted: "Last attempt interrupted",
  };
  const finished = m.last_job?.updated_at;
  const when = finished && status !== "running" && !m.notice ? " · " + utc(finished).slice(0, 8) + " UTC" : "";
  const target = state.info.targets.find(t => t.id === (m.last_job?.target_id || m.target_id));
  const label = target?.verification === "ncdxf_cw_v1" ? "IBP · " + frequency(target.frequency_hz) : "DCF77 · 77.50 kHz";
  const result = m.last_job?.sampled_slots != null ? ` · ${m.last_job.verified_slots}/${m.last_job.sampled_slots} slots verified` : "";
  if (m.last_jobs?.length > 1) {
    $("observation-title").textContent = m.notice || m.last_jobs.map(j => {
      const t = state.info.targets.find(t => t.id === j.target_id);
      return `${t ? frequency(t.frequency_hz) : j.target_id} · ${j.status}`;
    }).join(" / ");
  } else $("observation-title").textContent = label + " · " + (m.notice || messages[status] || "Waiting for a capture") + result + when;
  updateCountdown();
}
function updateCountdown() {
  if ($("monitor-clock").hidden) return;
  const next = Date.parse(state.data?.observatory?.next_at);
  if (!Number.isFinite(next)) return;
  const seconds = Math.ceil((next - nowUtc()) / 1000);
  const monitor = state.data.observatory;
  const ids = monitor.next_target_ids || [monitor.next_target_id];
  const label = ids.map(id => state.info.targets.find(t => t.id === id)).filter(Boolean).map(t => frequency(t.frequency_hz)).join(" + ") || "window";
  $("monitor-clock").textContent = seconds > 0
    ? `Next ${label} · ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`
    : "Checking window…";
}
function renderHistory() {
  if (state.mode !== "live" || !state.data) return;
  const rows = observations().filter(verified).slice(0, 120);
  const e = state.selected;
  const key = JSON.stringify([rows.map((r) => r.id), e?.id, state.pinned, state.followingBeacon, state.compareThree, $("follow-live").checked]);
  if (key === state.historyKey) return;
  state.historyKey = key;
  $("latest-fix").textContent = state.pinned || !$("follow-live").checked || state.manualClose ? "Follow latest" :
    state.followingBeacon && e ? "Following " + (isDcf(e) ? "DCF77" : name(e)) : "Following latest";
  $("latest-fix").setAttribute("aria-pressed", String(!state.pinned && $("follow-live").checked && !state.manualClose));
  const rowButton = (r) => `<button type="button" class="history-fix" data-fix="${esc(r.id)}" aria-pressed="${r.id === e?.id}" title="${esc(utcDate(r.start_utc))} UTC"><strong>${esc(isDcf(r) ? "DCF77" : name(r))} · ${esc(bandName(r))}</strong><span>${utc(r.start_utc).slice(0,8)} UTC · ${r.confirmed_count ?? r.receivers.filter(rx => rx.confirmed).length} RX</span><span>${r.estimate ? fmt(r.estimate.radius_km) + " km radius" : r.hypotheses?.length ? r.hypotheses.length + " hypotheses" : "undetermined"}</span></button>`;
  const focusId = document.activeElement?.dataset.fix;
  $("history-fixes").innerHTML = rows.map(rowButton).join("") || '<span class="history-empty">No verified beacon observation in this interval and band.</span>';
  for (const b of $("history-fixes").querySelectorAll("[data-fix]")) {
    b.onclick = () => inspect(b.dataset.fix);
    if (b.dataset.fix === focusId) b.focus({ preventScroll: true });
  }
  const three = e?.reference?.three_receivers;
  $("receiver-comparison").hidden = !three;
  const view = selectedView();
  $("fit-estimates").hidden = !(view?.estimate || view?.hypotheses?.length);
  $("three-rx").setAttribute("aria-pressed", String(state.compareThree));
  $("all-rx").setAttribute("aria-pressed", String(!state.compareThree));
  if (three) $("all-rx").textContent = `All ${usedIds(e).length} receivers`;
  let summary = "";
  if (verified(e)) {
    const previous = previousReference(e);
    if (state.compareThree) {
      const n = three.solutions.length;
      const result = n > 1 ? n + " compatible regions" : n === 1 ? "contour radius " + fmt(three.solutions[0].radius_km) + " km" : three.reason || "position undetermined";
      summary = `Same UTC samples · 3 of ${usedIds(e).length} receivers · ${result}.`;
    } else if (previous) {
      const changes = [];
      if (isIbp(e)) {
        const then = previous.confirmed_count ?? previous.receivers.filter(r => r.confirmed).length;
        const now = e.receivers.filter(r => r.confirmed).length;
        changes.push(`confirmed receptions ${then} → ${now}`);
      }
      if (e.estimate && previous.estimate) {
        const a = e.estimate.center, b = previous.estimate.center;
        const km = L.latLng(a.lat, a.lon).distanceTo([b.lat, b.lon]) / 1000;
        changes.push(`centres ${fmt(km)} km apart`, `contour radius ${fmt(previous.estimate.radius_km)} → ${fmt(e.estimate.radius_km)} km`);
      } else {
        changes.push("centre comparison unavailable for an undetermined position");
      }
      const added = usedIds(e).filter((id) => !usedIds(previous).includes(id)).length;
      if (added) changes.push(added + (added === 1 ? " receiver joined" : " receivers joined"));
      summary = `Compared with ${utc(previous.start_utc).slice(0,8)} UTC · ${changes.join(" · ")}.`;
    } else {
      summary = "First verified observation of this signal on this band in the selected interval.";
    }
  }
  $("fix-change").textContent = summary;
}
async function inspect(id, focusOnMap = false, followBeacon = false) {
  state.requestedStation = null;
  state.followingBeacon = null;
  $("station-notice").hidden = true;
  state.overview = false;
  state.manualClose = false;
  state.pinned = state.mode === "live";
  await selectEmission(id);
  if (focusOnMap && state.selected?.id === id) {
    const e = selectedView();
    if (followBeacon && state.mode === "live" && $("follow-live").checked && verified(e)) {
      state.followingBeacon = signalKey(e);
      state.pinned = false;
      renderHistory();
    }
    fitContours(e.estimate ? [e.estimate] : e.hypotheses);
    $("map").scrollIntoView({block:"nearest"});
  }
}
function nearLongitude(lon, anchor) {
  return lon + 360 * Math.round((anchor - lon) / 360);
}
function contourPoints(est) {
  const centre = nearLongitude(est.center.lon, map.getCenter().lng);
  return est.polygon.map(p => [p.lat, nearLongitude(p.lon, centre)]);
}
function renderRegions(emissions) {
  const current = selectedView();
  const previous = state.compareThree ? null : previousReference(current);
  const signals = state.data?.signals || [];
  const key = JSON.stringify([state.mode, state.scenario, emissions.map((r) => r.id), signals.map(r => [r.id, r.last_position?.id]), state.overview, current?.id, previous?.id, state.compareThree, receptionOn()]);
  if (key === state.regionKey) return;
  state.regionKey = key;
  regions.clearLayers();
  $("prior-legend").hidden = !(current?.estimate && previous?.estimate);
  if (current?.estimate && previous?.estimate) {
    const old = previous.estimate, a = old.center, b = current.estimate.center;
    L.polygon(contourPoints(old), {
      color: "#8bc6e2", weight: 2, opacity: 0.85, dashArray: "3 5", fill: false, className: "prior-contour",
    }).addTo(regions).bindTooltip("Previous contour · " + utc(previous.start_utc).slice(0,8) + " UTC");
    L.circleMarker([a.lat, a.lon], { radius: 4, color: "#8bc6e2", weight: 2, fillOpacity: 0.9 })
      .addTo(regions).bindTooltip("Previous estimate");
    L.polyline([[a.lat, nearLongitude(a.lon,b.lon)], [b.lat, b.lon]], { color: "#8bc6e2", weight: 2, dashArray: "2 3", interactive: false }).addTo(regions);
  }
  const displayed = state.mode === "live" ? [] : [...emissions];
  if (state.mode === "live") {
    displayed.push(...signals.filter(s => state.overview || signalKey(s) !== signalKey(current)));
    if (current && !state.overview) displayed.push(current);
    for (const s of signals) {
      if (!s.last_position) continue;
      if (!state.overview && signalKey(s) === signalKey(current) && (state.compareThree || s.id !== current.id)) continue;
      displayed.push({...s.last_position, laterReception: s.start_utc});
    }
    const sites = new Map();
    for (const signal of receptionOn() ? signals.filter(isIbp) : []) {
      if (signal.id === current?.id && !state.overview) continue;
      for (const r of signal.confirmed_receivers || []) {
        if (!sites.has(r.id)) sites.set(r.id, {r, heard:[]});
        sites.get(r.id).heard.push(signal);
      }
    }
    for (const {r,heard} of sites.values()) {
      const label = "RX heard " + heard.map(e => name(e) + " · " + bandName(e)).join(" / ");
      const marker = L.marker([r.position.lat, r.position.lon], {
        title: r.label + " · " + label, icon: L.divIcon({html:'<div class="heard-site"></div>',className:"",iconSize:[14,14],iconAnchor:[7,7]})
      }).addTo(regions);
      marker.bindTooltip(esc(label), {permanent:sites.size <= 6, direction:"top", offset:[0,-6], className:"heard-label"});
      marker.on("click", () => inspect(heard[0].id));
    }
  }
  for (const e of displayed) {
    const solutions = e.estimate ? [e.estimate] : e.hypotheses || [];
    const timeLabel = e.laterReception ? ` · last TDoA ${utc(e.start_utc).slice(0,8)} UTC · heard again ${utc(e.laterReception).slice(0,8)} UTC` : "";
    solutions.forEach((est, index) => {
      const ambiguous = !e.estimate;
      const polygon = L.polygon(
        contourPoints(est),
        {
          color: ambiguous ? "#e7b879" : "#c0ef85",
          weight: 1.6,
          fillColor: ambiguous ? "#e7b879" : "#b2df7a",
          fillOpacity: 0.16,
          dashArray: ambiguous || e.laterReception || !verified(e) ? "5 4" : null,
          className: verified(e) ? "observation-contour" : "",
        },
      ).addTo(regions);
      polygon.on("click", () => inspect(e.id));
      if (ambiguous) {
        polygon.bindTooltip("Hypothesis " + (index + 1) + " · " + esc(name(e)) + (isIbp(e) ? " · " + esc(bandName(e)) : "") + timeLabel, {
          permanent: true,
          direction: "bottom",
          offset: [0, 12],
          className: "tx-label ambiguous-label",
        });
      } else {
        const p = est.center;
        L.marker([p.lat, p.lon], {
          title: "TDoA transmitter · " + name(e), alt: "TDoA transmitter · " + name(e),
          icon: L.divIcon({
            html: '<div class="tx-icon"></div>',
            className: "",
            iconSize: [17, 17],
            iconAnchor: [8, 8],
          }),
        })
          .addTo(regions)
          .bindTooltip("TX · " + esc(name(e) + (isIbp(e) ? " · " + bandName(e) : "") + (timeLabel || (isIbp(e) ? " · " + utc(e.start_utc).slice(0,8) + " UTC" : ""))), {
            permanent: true,
            direction: "bottom",
            offset: [0, 15],
            className: "tx-label",
          })
          .on("click", () => inspect(e.id));
      }
    });
  }
}
async function selectEmission(id) {
  const request = ++state.selectionRequest;
  try {
    const e = await api("/api/emissions/" + encodeURIComponent(id));
    if (request !== state.selectionRequest) return;
    if (e.id !== state.selected?.id) state.compareThree = false;
    state.selected = e;
    $("empty-map").hidden = true;
    renderSelected();
    drawSpectrum(e.spectrum, e.frequency_hz);
    if (state.data) renderList(state.data);
    if (state.fitFirst) {
      state.fitFirst = false;
      if (state.mode === "live" && !receptionOn()) fitContours(e.estimate ? [e.estimate] : e.hypotheses);
      else fitNetwork();
    }
    icons();
  } catch (err) {
    if (request === state.selectionRequest) notify(err.message);
  }
}
function renderSelected(previousIds) {
  const view = selectedView();
  if (view) {
    $("map-note").hidden = true;
    renderInspector(view);
    renderNetwork(view, previousIds);
  }
  if (state.data) renderRegions(state.data.emissions);
  renderHistory();
}
function clearSelection() {
  state.selectionRequest++;
  state.selected = null;
  state.followingBeacon = null;
  state.compareThree = false;
  state.regionKey = null;
  state.historyKey = null;
  $("inspector").hidden = true;
  network.clearLayers();
  drawSpectrum([]);
}
function renderNetwork(e, previousIds) {
  network.clearLayers();
  if (state.overview || (state.mode === "live" && !receptionOn())) return;
  const previous = previousReference(e);
  const priorIds = previousIds ?? (!state.compareThree && previous ? usedIds(previous) : null);
  for (const r of e.receivers) {
    if (!r.confirmed) continue;
    const p = r.position;
    const marker = L.marker([p.lat, p.lon], {
      icon: L.divIcon({
        html: `<div class="rx-icon ${r.used ? "" : "unused"} ${r.used && priorIds && !priorIds.includes(r.id) ? "rx-joined" : ""}"></div>`,
        className: "",
        iconSize: [12, 12],
        iconAnchor: [6, 6],
      }),
    }).addTo(network);
    const city = r.label.split("|").at(-1).split(/,| - /)[0].trim().replace(/^\d{4,6}\s+/, "");
    const short = city.length > 20 ? city.slice(0, 19) + "…" : city;
    marker.bindTooltip(esc(verified(e) ? short || r.id : r.label), {
      permanent: state.mode !== "live" || map.getZoom() >= 4,
      direction: "top",
      offset: [0, -10],
      className: "rx-label",
    });
    marker.on("click", () => receiverProof(r));
    if (e.estimate && (!state.compareThree || r.used))
      L.polyline(
        [
          [p.lat, nearLongitude(p.lon,e.estimate.center.lon)],
          [e.estimate.center.lat, e.estimate.center.lon],
        ],
        {
          color: r.used ? "#accc8b" : "#bfa47d",
          weight: 1,
          opacity: 0.5,
          dashArray: "4 7",
          interactive: false,
        },
      ).addTo(network);
  }
}
function renderInspector(e) {
  if (state.overview) { $("inspector").hidden = true; return; }
  $("inspector").hidden = false;
  const est = e.estimate,
    confirmed = e.receivers.filter((r) => r.confirmed),
    used = e.receivers.filter((r) => r.used);
  let reason = e.reason;
  if (isIbp(e) && !state.compareThree && !est && !e.hypotheses.length && confirmed.length >= 3 && e.pairs.length) {
    const passed = e.pairs.filter(p => p.accepted).length;
    if (e.pairs.every(p => p.sigma_us > 1500)) {
      const scatter = e.pairs.map(p => p.sigma_us / 1000);
      reason = `Keying-edge scatter ${fmt(Math.min(...scatter),2)}–${fmt(Math.max(...scatter),2)} ms exceeds the 1.50 ms pair limit.`;
    }
    reason = `${passed}/${e.pairs.length} accepted delay pairs. ${reason || ""}`;
  }
  const b = map.getBounds();
  $("inspector").innerHTML =
    `<div class="inspector-head"><div class="inspector-heading-row"><span>SELECTED EMISSION</span><button id="close-inspector" aria-label="Close details">×</button></div><div class="signal-name"><i data-lucide="radio"></i>${esc(name(e))}</div><div class="identity-note">${state.compareThree ? "Three-receiver comparison · same UTC samples" : isIbp(e) ? "Scheduled callsign matched in recorded Morse code" : e.identity ? "Identity documented for this recording" : isDcf(e) ? "Phase code verified · callsign unassigned" : "No verifiable identity associated"}</div></div><div class="inspector-values"><div><div class="metric-label">FREQUENCY</div><div class="metric-value">${frequency(e.frequency_hz)}</div></div><div><div class="metric-label">CAPTURE UTC</div><div class="metric-value">${utc(e.start_utc).slice(0, 8)}</div></div><div><div class="metric-label">UNCERTAINTY / MODEL</div><div class="metric-value green">${est ? "≤ " + fmt(est.radius_km) + " km" : e.hypotheses?.length ? e.hypotheses.length + " hypotheses" : "—"}</div><div class="metric-foot">${est ? "Maximum contour radius" : "Position undetermined"}</div></div><div><div class="metric-label">RECEIVERS USED</div><div class="metric-value">${used.length} <span class="dim">/ ${confirmed.length}</span></div><div class="metric-foot">${fmt((new Date(e.end_utc) - new Date(e.start_utc)) / 1000, 1)} s simultaneous</div></div></div><div class="quality-strip ${est ? "" : "warning"}"><i data-lucide="${est ? "circle-check" : "triangle-alert"}"></i><span>${est ? "GNSS + correlation + geometry validated" : e.hypotheses?.length ? e.hypotheses.length + " compatible global solutions · Position undetermined" : `Position undetermined · ${esc(reason)}`}</span></div><div class="rx-section"><div class="rx-heading"><span>All confirmed receptions</span><span>${confirmed.length} RX</span></div>${confirmed.map((r) => `<div class="rx-row"><span class="mini-rx ${r.used ? "" : "unused"}"></span><button data-rx="${esc(r.id)}">${esc(r.label)}</button>${b.contains([r.position.lat, r.position.lon]) ? "" : '<span class="outside">outside view</span>'}<span class="rx-state ${r.used ? "" : "unused"}">${r.used ? "USED" : !r.synchronized ? "NO GNSS" : isIbp(e) ? "HEARD" : "EXCLUDED"}</span></div>`).join("") || '<p class="caption muted">No confirmed correlated reception.</p>'}</div><div class="inspector-footer"><button id="proof"><i data-lucide="file-check-2"></i>Evidence and correlations</button><button id="export"><i data-lucide="download"></i>JSON</button></div>`;
  if (est) {
    const coordinates = document.createElement("p");
    coordinates.className = "tx-coordinates";
    coordinates.textContent = `TDoA centre · ${fmt(est.center.lat,2)}° latitude, ${fmt(est.center.lon,2)}° longitude`;
    $("inspector").querySelector(".inspector-values").after(coordinates);
  }
  $("close-inspector").onclick = () => {
    state.manualClose = true;
    clearSelection();
    refresh();
  };
  $("proof").onclick = () => showProof(state.selected);
  $("export").onclick = () => {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(state.selected, null, 2)], { type: "application/json" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = e.id + ".json";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  document
    .querySelectorAll("[data-rx]")
    .forEach(
      (btn) =>
        (btn.onclick = () =>
          receiverProof(e.receivers.find((r) => r.id === btn.dataset.rx))),
    );
  icons();
}
function drawSpectrum(data, freq) {
  const ticks = document.querySelectorAll(".spectrum-ticks span");
  const left = data?.length ? data[0][0] / 1000 : -6;
  const right = data?.length > 1 ? (data.at(-1)[0] + data[1][0] - data[0][0]) / 1000 : 6;
  const khz = n => (n < 0 ? "−" : "+") + fmt(Math.abs(n), Number.isInteger(n) ? 0 : 1) + " kHz";
  ticks[0].textContent = khz(left);
  ticks[1].textContent = "Offset from channel";
  ticks[2].textContent = khz(right);
  $("spectrum-freq").textContent = freq ? frequency(freq) : "NO RECORDING";
  let html =
    '<defs><linearGradient id="spec-fill" x1="0" x2="0" y1="0" y2="1"><stop stop-color="#b5de80" stop-opacity=".24"/><stop offset="1" stop-color="#b5de80" stop-opacity="0"/></linearGradient></defs>';
  for (let y = 10; y < 80; y += 20)
    html += `<path d="M0 ${y}H600" stroke="#2c3946" stroke-dasharray="2 4" stroke-width=".6"/>`;
  if (data?.length) {
    const values = data.map((x) => x[1]),
      min = Math.min(...values) - 3,
      max = Math.max(...values) + 3;
    const points = data
      .map(
        (p, i) =>
          `${(i / (data.length - 1)) * 600},${72 - ((p[1] - min) / (max - min)) * 65}`,
      )
      .join(" ");
    html += `<polygon points="0,80 ${points} 600,80" fill="url(#spec-fill)"/><polyline points="${points}" fill="none" stroke="#b3d984" stroke-width="1.2"/>`;
  } else
    html +=
      '<text x="300" y="45" text-anchor="middle" fill="#64778b" font-size="10">No IQ samples in this selection</text>';
  $("spectrum").innerHTML = html;
}
function renderCoverage(m) {
  const none = m.status === "unmonitored",
    noSignal =
      m.segments.length && m.segments.every((s) => s.status === "no_detection");
  $("coverage-title").innerHTML =
    `<span class="${none ? "amber-dot" : "status-dot"}"></span> ${none ? "Unmonitored interval" : noSignal ? "No emission detected" : m.gaps.length ? "Partial monitoring" : "Recorded channels"}`;
  $("coverage-detail").textContent = none
    ? (state.mode === "live" ? "No processed receiver data covers this band and interval." : "No recordings cover this interval. No conclusion about the absence of emissions is possible.")
    : `${m.gaps.length ? "Parts of this interval were not monitored. " : ""}Coverage applies to the sampled channels and receivers.`;
  $("capture-window").textContent = m.segments.length
    ? `${utc(m.segments[0].start).slice(0, 8)} — ${utc(m.segments[0].end).slice(0, 8)} UTC · ${frequency(m.segments[0].frequency_hz)}`
    : "—";
  const track = $("coverage-track");
  track.toggleAttribute("hidden", state.mode !== "live");
  if (state.mode !== "live") return;
  const start = Date.parse($("start").value + "Z"), end = Date.parse($("end").value + "Z");
  const scale = (t) => Math.max(0, Math.min(600, 600 * (Date.parse(t) - start) / (end - start)));
  track.innerHTML = '<title>Green segments show processed IQ, including slots with unverified callsigns. Dark segments are gaps between samples.</title>' +
    m.segments.map((s) => `<rect x="${scale(s.start)}" width="${scale(s.end) - scale(s.start)}" height="12"><title>${utc(s.start).slice(0,8)}–${utc(s.end).slice(0,8)} UTC</title></rect>`).join("");
}
function fitNetwork() {
  const e = selectedView();
  if (!e) return;
  const confirmed = e.receivers.filter(r => r.confirmed);
  const anchor = e.estimate?.center.lon ?? confirmed[0]?.position.lon ?? map.getCenter().lng;
  const points = confirmed.map(r => [r.position.lat, nearLongitude(r.position.lon, anchor)]);
  if (e.estimate) points.push(...e.estimate.polygon.map(p => [p.lat, nearLongitude(p.lon, anchor)]));
  else if (state.compareThree) points.push(...e.hypotheses.flatMap(contourPoints));
  if (points.length)
    map.fitBounds(points, {
      paddingTopLeft: [65, 80],
      paddingBottomRight: window.innerWidth > 680 ? [290, 70] : [30, 70],
      maxZoom: 7,
    });
}
function openModal(title, html) {
  $("modal-title").textContent = title;
  $("modal-content").innerHTML = html;
  if (!$("modal").open) $("modal").showModal();
  icons();
}
function receiverProof(r) {
  openModal(
    "Receiver · " + r.label,
    `<h3>Reception and synchronization</h3><p>${r.confirmed ? (isIbp(state.selected) ? "Full scheduled callsign verified in this receiver’s IQ." : "RF correlation confirmed.") : "Reception of the same emission was not confirmed."} ${r.used ? "Receiver used in the calculation." : "Receiver excluded from the calculation."}</p><p>${esc(r.reason || "Continuous GNSS timestamps; solution age ≤ 2 s in the analysed segment.")}</p><table><tr><td>Published position</td><td>${fmt(r.position.lat, 6)}°, ${fmt(r.position.lon, 6)}°</td></tr><tr><td>Sample rate measured from GNSS</td><td>${fmt(r.sample_rate_hz, 3)} Hz</td></tr><tr><td>Maximum GNSS age</td><td>${r.gps_age_max_s === 255 ? "No solution" : r.gps_age_max_s + " s"}</td></tr><tr><td>Samples in the common interval</td><td>${r.samples}</td></tr></table><h3>Position provenance</h3><p>${esc(r.position_evidence.note)}</p><p>${link(r.position_evidence.url, "Coordinate source ↗")}</p><h3>Recording integrity</h3><p class="proof"><code>${esc(r.sha256)}</code></p><p>Reception applies to the displayed UTC interval.</p>`,
  );
}
function showProof(e) {
  const cwProof = isIbp(e) ? `<h3>CW verification in each receiver</h3><table><tr><th>Receiver</th><th>Best full call</th><th>Score / runner-up</th><th>Verified</th></tr>${e.reference.cw.map(r =>
    `<tr><td>${esc(r.receiver_id)}</td><td>${esc(r.best_call)}</td><td>${fmt(r.score,2)} / ${fmt(r.runner_up,2)}</td><td>${r.accepted ? "Yes" : "No"}</td></tr>`).join("")}</table>` : "";
  openModal(
    "Evidence · " + name(e),
    `<span class="badge ${e.fault_injected ? "test" : e.mode === "live" ? "live" : "replay"}">${e.fault_injected ? "RECORDING TEST" : e.mode === "live" ? "LIVE" : "REAL RECORDING"}</span><p>${esc(e.provenance.note)}</p><p>${link(e.provenance.url, "Recording source ↗")}</p><h3>Identity</h3><p>${e.identity ? esc(e.identity.evidence.note) : "unknown callsign"}</p>${e.identity ? link(e.identity.evidence.url, "Evidence linking identity to this emission ↗") : ""}${cwProof}<h3>Measured delays</h3><table><thead><tr><th>Pair</th><th>Δt (µs)</th><th>Timing σ (ms)</th><th>Correlation</th><th>${isIbp(e) ? "Matched key edges" : "Windows"}</th></tr></thead><tbody>${e.pairs.map((p) => `<tr><td>${esc(p.a)} → ${esc(p.b)}</td><td>${fmt(p.delay_us, 2)}</td><td>${fmt(p.sigma_us/1000, 2)}</td><td>${fmt(p.coherence, 3)}</td><td>${p.windows} ${p.accepted ? "✓" : "×"}</td></tr>`).join("")}</tbody></table><p>Δt = arrival B − arrival A. Normalized correlation is not a probability. Unsynchronized receivers may have correlated reception but are excluded from localization.</p><h3>Uncertainty and limitations</h3><p>${esc(e.estimate?.uncertainty_note || e.reason)}</p>${e.hypotheses?.length ? '<p>These contours represent alternative hypotheses. The transmitter reference is never used to select one.</p><button class="primary-button" id="all-hypotheses">Show all hypotheses on the map</button>' : ""}<p>Published receiver coordinates do not establish exact antenna offsets. Spherical propagation is an approximation; the result does not guarantee accuracy.</p><h3>Separate evaluation</h3><p>The known transmitter position is not used by the solver, receiver selection or search domain.</p><button class="primary-button" id="show-evaluation">View real-data evaluation</button>`,
  );
  const hasEvaluation = verified(e) || e.id === "dcf77-20200813";
  $("show-evaluation").hidden = !hasEvaluation;
  $("show-evaluation").onclick = () => evaluation(e);
  if ($("all-hypotheses"))
    $("all-hypotheses").onclick = () => {
      $("modal").close();
      map.fitBounds(
        e.hypotheses.flatMap(contourPoints),
        { padding: [60, 60] },
      );
    };
}
async function evaluation(e) {
  try {
    if (e?.mode === "live") {
      if (!verified(e)) return;
      const ref = await api("/api/emissions/" + encodeURIComponent(e.id) + "/evaluation");
      const rows = ref.solutions.map((s, i) =>
        `<tr><td>${ref.unique ? "Full network" : "Hypothesis " + (i + 1)}</td><td>${fmt(s.error_km,2)} km${s.error_range_km ? `<br>Cell bounds ${s.error_range_km.map(v => fmt(v)).join("–")} km` : ""}</td><td>${fmt(s.model_radius_km,1)} km</td></tr>`);
      openModal("Live observation evaluation", `<span class="badge live">${esc(bandName(e))} · ${esc(isDcf(e) ? "DCF77" : name(e))}</span>
        <h3>${utcDate(e.start_utc)} · ${utc(e.start_utc).slice(0,8)} UTC</h3>
        <p>${usedIds(e).length} contributing receivers · ${frequency(e.frequency_hz)}. ${ref.grid ? `Distances use the centre of published locator ${esc(ref.grid)}. Its farthest corner is ${fmt(ref.grid_radius_km)} km from the centre.` : "Distances use the published PTB position."}</p>
        ${rows.length ? '<table><tr><th>Solution</th><th>Distance to reference</th><th>Model radius</th></tr>' + rows.join("") + '</table>' : '<p>This capture has no accepted position to evaluate.</p>'}
        ${rows.length > 1 ? '<p>All compatible hypotheses are listed. The band accuracy table counts unique fixes.</p>' : ""}
        <p>${link(ref.reference_source, ref.grid ? "NCDXF/IARU published locators ↗" : "Position published by PTB ↗")}</p>`);
      return;
    }
    const r = await api("/validation.json");
    openModal(
      "Real-recording evaluation",
      `<span class="badge replay">${esc(r.dataset)}</span><h3>${r.unique ? "Unique position" : "Position undetermined · global ambiguity"}</h3><p>Each hypothesis is compared with the PTB reference only after the solver finishes. The reference is never used to choose a solution.</p><table><tr><th>Hypothesis</th><th>Estimate</th><th>Measured error</th></tr>${r.hypotheses.map((h, i) => `<tr><td>${i + 1}</td><td>${fmt(h.center.lat, 5)}°, ${fmt(h.center.lon, 5)}°</td><td>${fmt(h.error_km, 2)} km</td></tr>`).join("")}</table><p>There is no single accepted localization error when the solution is ambiguous. A hypothesis close to the beacon is not a uniquely validated result.</p><table><tr><td>Evaluation reference</td><td>${fmt(r.reference.lat, 6)}°, ${fmt(r.reference.lon, 6)}°</td></tr><tr><td>GNSS loss</td><td>${esc(r.sync_loss_result)}</td></tr></table><p>${link(r.reference_source, "Position published by PTB ↗")}</p><p>${esc(r.limitations)}</p>`,
    );
  } catch (err) {
    openModal(
      "Evaluation unavailable",
      `<p>${esc(err.message)}. The report has not been generated yet.</p>`,
    );
  }
}
function methodology() {
  openModal(
    "Methodology and provenance",
    `<h3>01 / Independent capture</h3><p>Independent SDRs record IQ with GNSS timestamps. A TDoA position requires at least three synchronized receivers for the same emission. Live capture checks API authorization, validity, bands, antenna location and receiver status. A public listing alone does not establish permission.</p><h3>02 / Same signal, common timebase</h3><p>SHA-256, Kiwi WAV blocks, GNSS continuity and age are checked. The first settling second is excluded. Streams are resampled onto a common interval. DCF77 uses its phase code and two-second correlation windows. Each HF beacon slot is checked against all 18 callsign patterns. Pairwise timing comes from matching measured keying edges in independent recordings.</p><h3>03 / TDoA only</h3><p>Position is derived from time differences, with geometry and alternative solutions checked. RSSI, callsigns and known transmitter positions are not localization inputs. Contours show model-dependent uncertainty; statistical coverage is not calibrated.</p><h3>04 / Explicit limits</h3><p>Visible receivers do not establish geographic coverage. “No emission detected” only applies to the recorded channel and interval under the documented detector thresholds. Other intervals remain unmonitored. DCF77 is a public time reference used for commissioning, not an amateur station.</p><h3>Primary documentation</h3><p>${link("https://www.ncdxf.org/beacon/beaconcontroller.html", "NCDXF/IARU · beacon timing and keying ↗")}<br>${link("https://github.com/jks-prv/kiwiclient", "KiwiClient · capture and IQ format ↗")}<br>${link("https://github.com/hcab14/TDoA", "TDoA · original recording and receiver coordinates ↗")}<br>${link("https://www.ptb.de/cms/fileadmin/internet/publikationen/masstaebe/Hefte_Komplett_PDF/mst01.pdf", "PTB · DCF77 reference ↗")}</p><button class="primary-button" id="method-eval">Evaluation report</button>`,
  );
  $("method-eval").hidden = state.mode === "live" && !verified(state.selected);
  $("method-eval").onclick = () => evaluation(state.mode === "live" ? state.selected : null);
}
function recordings() {
  openModal(
    "Recording archive",
    state.info.datasets
      .map(
        (m) =>
          `<div class="dataset-card"><span class="badge replay">REAL IQ · REPLAY</span><h3>${esc(m.title)}</h3><p>${utcDate(m.capture_utc)} · ${utc(m.capture_utc).slice(0, 8)} UTC · ${frequency(m.frequency_hz)}<br>${m.receivers.length} receivers · GNSS timestamps in the files</p><p>${esc(m.provenance.note)}</p><button class="primary-button" data-dataset="${esc(m.id)}">Open recording</button></div>`,
      )
      .join("") || "<p>No recordings imported.</p>",
  );
  document.querySelectorAll("[data-dataset]").forEach(
    (btn) =>
      (btn.onclick = () => {
        state.datasetId = btn.dataset.dataset;
        $("modal").close();
        resetFilters("replay");
      }),
  );
}
const selectedSources = new Set();
let catalogOffset = 0, catalogSearch = "", catalogEligible = false;
async function sources(offset = 0) {
  catalogOffset = typeof offset === "number" ? offset : 0;
  state.info = await api("/api/info");
  openModal("Public SDR receivers", "<p>Reading the public directory…</p>");
  try {
    const d = await api("/api/sources?" + new URLSearchParams({offset: catalogOffset, limit: 30, search: catalogSearch, eligible: catalogEligible}));
    const live = state.info.sources;
    const mapped = new Set(state.info.live_enabled && state.beacons
      ? [...state.beacons.receiver_pool, ...state.beacons.other_receivers].map(r => r.id) : []);
    openModal("Public SDR receivers", `
      <span class="badge ${state.info.live_enabled ? "live" : "replay"}">${state.info.live_enabled ? "CAPTURE ON DEMAND" : "CAPTURE DISABLED"}</span>
      <p>Catalog: <strong>${d.total ?? 0}</strong>receivers · published API allowance: <strong>${d.api_allowed ?? 0}</strong> ·TDoA candidates: <strong>${d.candidates ?? 0}</strong>.</p>
      <p>${d.fetched_at ? "Updated at " + esc(d.fetched_at) + (d.stale ? " · STALE METADATA" : "") : "The directory is not available yet."} ${esc(d.note)}</p>
      <p>A candidate is not a confirmed reception. Selected receivers are checked directly before each capture; GNSS and correlation are then verified in the IQ data.</p>
      <form id="catalog-form"><label for="catalog-search">Search receiver / antenna</label><input id="catalog-search" value="${esc(catalogSearch)}" maxlength="100"><label class="catalog-check"><input type="checkbox" id="catalog-eligible" ${catalogEligible ? "checked" : ""}> TDoA candidates only</label><button class="primary-button">Search directory</button></form>
      <p id="catalog-selection">Selected: ${selectedSources.size}/8</p><p id="catalog-error" role="alert"></p>
      <div class="catalog-list">${(d.receivers || []).map(({receiver: r, exclusion}) => `<div class="dataset-card"><label class="catalog-check"><input type="checkbox" data-source-id="${esc(r.id)}" ${selectedSources.has(r.id) ? "checked" : ""} ${exclusion ? "disabled" : ""}><strong>${esc(r.name)}</strong></label><p>${esc(r.id)} · ${exclusion ? esc(exclusion) : "Candidate · IQ not verified"}<br>API: ${r.ext_api ?? "unpublished"}· channels ${r.users}/${r.users_max} · GNSS ${r.gps_good}<br>${esc(r.antenna)}<br>Published position: ${r.gps ? r.gps.map(n => fmt(n, 5)).join("°, ") + "°" : "unknown"}</p>${mapped.has(r.id) ? `<a href="?receiver=${esc(encodeURIComponent(r.id))}">Show on map</a> · ` : ""}${link(r.url, "Operator website ↗")}</div>`).join("")}</div>
      <div class="catalog-pages"><button class="primary-button" id="catalog-prev" ${catalogOffset ? "" : "disabled"}>Previous</button><span>${d.matching ? catalogOffset + 1 : 0}–${catalogOffset + (d.receivers?.length || 0)} / ${d.matching ?? 0}</span><button class="primary-button" id="catalog-next" ${d.next_offset == null ? "disabled" : ""}>Next</button></div>
      <button class="primary-button" id="catalog-capture">Capture with selected receivers</button>
      <h3>Configured target receivers</h3><p>${live.length ? live.map(s => esc(s.label)).join(" · ") : "No SDR operator configured for a target. Select directory candidates for a configured eligible target."}</p>
      ${(state.info.pending_captures || []).map(m => `<div class="dataset-card"><strong>Capture awaiting RF verification</strong><p>${esc(m.id)} · ${esc(m.capture_utc)}</p><button class="primary-button" data-review="${esc(m.id)}">Verify emission</button></div>`).join("")}
      <p>Antenna positions are operator-reported; exact antenna and cable offsets have not been independently measured.</p>`);
    $("catalog-form").onsubmit = e => { e.preventDefault(); catalogSearch = $("catalog-search").value; catalogEligible = $("catalog-eligible").checked; sources(0); };
    $("catalog-prev").onclick = () => sources(Math.max(0, catalogOffset - 30));
    $("catalog-next").onclick = () => sources(d.next_offset);
    document.querySelectorAll("[data-source-id]").forEach(input => input.onchange = () => {
      if (input.checked && selectedSources.size >= 8) { input.checked = false; $("catalog-error").textContent = "Maximum 8 receivers per capture."; return; }
      input.checked ? selectedSources.add(input.dataset.sourceId) : selectedSources.delete(input.dataset.sourceId);
      $("catalog-selection").textContent = `Selected: ${selectedSources.size}/8`;
      $("catalog-error").textContent = "";
    });
    document.querySelectorAll("[data-review]").forEach(b => b.onclick = () => reviewCapture(b.dataset.review));
    $("catalog-capture").onclick = () => {
      if (selectedSources.size && selectedSources.size < 3) { $("catalog-error").textContent = "At least three independent receivers are required."; return; }
      if (!state.info.live_enabled || !state.info.targets.length) { $("catalog-error").textContent = "Capture requires live_enabled and a configured eligible target."; return; }
      setMode("live"); captureDialog();
    };
  } catch (e) { openModal("Directory unavailable", `<p>${esc(e.message)}</p><p>This does not indicate an absence of RF emissions.</p>`); }
}
function setBusy(b) {
  state.busy = b;
  $("replay").disabled = b;
  $("test-sync").disabled = b;
}
async function runJob(fault = false) {
  if (state.mode === "live") {
    captureDialog();
    return;
  }
  const dataset =
    state.info.datasets.find((d) => d.id === state.datasetId) ||
    state.info.datasets[0];
  if (!dataset) return;
  setBusy(true);
  notify(
    fault
      ? "Loading the real-IQ GNSS-loss test result…"
      : "Loading the IQ processing result and correlations…",
  );
  try {
    const job = await api("/api/replay", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        dataset_id: dataset.id,
        fault_receiver: fault ? dataset.receivers.at(-1).id : null,
      }),
    });
    const completed = await waitJob(job.job_id);
    state.scenario = fault ? "sync-loss" : "normal";
    state.manualClose = false;
    clearSelection();
    updateModeLabels();
    await refresh(true);
    fitNetwork();
    notify(
      fault
        ? "Test complete: check that localization is refused without synchronization."
        : completed.cached ? "Loaded the verified IQ processing result from the archive." : "The recording was reprocessed from IQ.",
    );
  } catch (err) {
    notify(err.message);
  } finally {
    setBusy(false);
  }
}
async function waitJob(id) {
  for (let i = 0; i < 180; i++) {
    const j = await api("/api/jobs/" + id);
    if (j.status === "completed" || j.status === "awaiting_review") return j;
    if (j.status === "failed" || j.status === "interrupted") throw new Error(j.error);
    await new Promise((r) => setTimeout(r, 700));
  }
  throw new Error(
    "Processing continues on the server; refresh the list for the result.",
  );
}
function captureDialog() {
  if (!state.info.live_enabled || !state.info.targets.length) {
    sources();
    return;
  }
  openModal(
    "Capture on the approved network",
    `<p>${selectedSources.size ? `${selectedSources.size} receivers selected from the directory.` : "Using the receivers configured for this target."} Each connection respects the published API policy. Synchronization and signal eligibility are verified in the IQ data.</p><label for="capture-target">Approved target</label><select id="capture-target">${state.info.targets.map((t) => `<option value="${esc(t.id)}">${esc(t.id)} · ${frequency(t.frequency_hz)}</option>`).join("")}</select><label for="admin-token">Administrator token</label><input type="password" id="admin-token" autocomplete="off"><button class="primary-button" id="capture-submit">Start capture</button><p id="capture-error" role="alert"></p>`,
  );
  $("capture-submit").onclick = async () => {
    setBusy(true);
    $("capture-submit").disabled = true;
    try {
      const j = await api("/api/capture", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer " + $("admin-token").value,
        },
        body: JSON.stringify({ target_id: $("capture-target").value, source_ids: selectedSources.size ? [...selectedSources] : null }),
      });
      $("admin-token").value = "";
      $("modal").close();
      notify("Capture started on approved receivers.");
      const result = await waitJob(j.job_id);
      if (result.status === "awaiting_review") {
        reviewCapture(result.capture_id);
      } else {
        await refresh(true);
        fitNetwork();
      }
    } catch (err) {
      if ($("capture-error")) $("capture-error").textContent = err.message;
      notify(err.message);
    } finally {
      setBusy(false);
      if ($("capture-submit")) $("capture-submit").disabled = false;
    }
  };
}
function updateModeLabels() {
  const replay = state.mode === "replay",
    test = state.scenario === "sync-loss";
  $("follow-live-label").hidden = replay;
  $("live-history").hidden = replay;
  $("transmitters").hidden = replay;
  $("beacon-network").hidden = replay || !worldwide();
  $("map-activity").hidden = true;
  $("map-method").hidden = false;
  $("monitor-clock").hidden = true;
  $("replay").hidden = false;
  $("back-recording").hidden = !replay;
  $("page-title").textContent = replay ? "Recording explorer" : "Live radio observatory";
  $("page-description").textContent = replay ? "Explore an observation using synchronized receiver recordings." : "Select a transmitter to see its measured position, uncertainty and reception time.";
  $("mode-replay").classList.toggle("selected", replay);
  $("mode-live").classList.toggle("selected", !replay);
  $("mode-badge").className =
    "badge " + (test ? "test" : replay ? "replay" : "live");
  $("mode-badge").textContent = test
    ? "TEST · GNSS LOSS"
    : replay
      ? "REAL RECORDING"
      : "LIVE";
  $("observation-title").textContent = replay
    ? `${test ? "Injected fault · " : ""}${utcDate((state.info.datasets.find((d) => d.id === state.datasetId) || state.info.datasets[0])?.capture_utc || "2020-08-13")} · ${esc((state.info.datasets.find((d) => d.id === state.datasetId) || state.info.datasets[0])?.kind === "public_time_reference" ? "public time reference" : "documented eligible emission")}`
    : "Approved network · " +
      (state.info.live_enabled
        ? "capture on demand"
        : "capture is not configured");
  $("replay").innerHTML =
    `<i data-lucide="${replay ? "rotate-cw" : "radio"}"></i>${replay ? "IQ result" : "New capture"}`;
  $("test-sync").hidden = !replay;
  $("analysis-subtitle").textContent = test
    ? "TEST · unchanged IQ"
    : "UTC · recorded channel";
  icons();
}
function liveInterval() {
  const now = nowUtc();
  $("start").value = new Date(now - 3600000).toISOString().slice(0, 19);
  $("end").value = new Date(now + 1000).toISOString().slice(0, 19);
}
function resetFilters(mode = state.mode, reception = false, station = null, band = "all") {
  $("toast").hidden = true;
  $("station-notice").hidden = true;
  state.requestedStation = station;
  state.mode = mode;
  state.scenario = "normal";
  state.manualClose = false;
  state.pinned = false;
  state.fitFirst = mode === "live" && !worldwide();
  const bandInput = $("band");
  bandInput.value = [...bandInput.options].some(option => option.value === band) ? band : "all";
  state.overview = mode === "live" && worldwide() && bandInput.value === "all";
  $("show-reception").checked = mode === "live" && reception;
  state.data = null;
  state.historyKey = null;
  $("follow-live").checked = true;
  if (mode === "live") {
    liveInterval();
  } else {
    const d = state.info.datasets.find((x) => x.id === state.datasetId) || state.info.datasets[0];
    const start = d?.capture_utc || "2020-08-13T06:52:20Z";
    $("start").value = start.slice(0, 19);
    $("end").value = new Date(Date.parse(start) + 35000).toISOString().slice(0, 19);
  }
  clearSelection();
  renderRegions([]);
  updateModeLabels();
  updateReceptionControls();
  if (mode === "live" && (worldwide() || state.info.digital_reports)) worldView(); else map.setView([49.5, 6.5], 5);
  drawPool();
  drawSchedule();
  digital.refresh();
  refresh(true).then(() => { if (state.mode === "replay") fitNetwork(); });
}
function setMode(mode) {
  if (mode !== state.mode) resetFilters(mode);
}
function followLatest() {
  state.requestedStation = null;
  state.followingBeacon = null;
  $("station-notice").hidden = true;
  state.overview = false;
  state.regionKey = null;
  if (state.selected) renderSelected();
  state.pinned = false;
  state.manualClose = false;
  $("follow-live").checked = true;
  liveInterval();
  refresh(true);
}
$("latest-fix").onclick = followLatest;
$("fit-estimates").onclick = () => {
  const e = selectedView();
  if (!e) return;
  const solutions = e.estimate ? [e.estimate] : [...e.hypotheses];
  const previous = previousReference(e);
  if (!state.compareThree && e.estimate && previous?.estimate) solutions.push(previous.estimate);
  fitContours(solutions);
};
function fitContours(solutions) {
  if (solutions?.length) map.fitBounds(solutions.flatMap(contourPoints), {
    animate:false, paddingTopLeft:[40,65], paddingBottomRight:innerWidth > 680 && !$("inspector").hidden ? [290,65] : [40,65], maxZoom:13,
  });
}
$("follow-live").onchange = () => {
  if ($("follow-live").checked) followLatest();
  else renderHistory();
  digital.refresh();
};
for (const [id, three] of [["three-rx", true], ["all-rx", false]]) {
  $(id).onclick = () => {
    if (!state.selected?.reference?.three_receivers) return;
    const before = !three && state.compareThree ? state.selected.reference.three_receivers.receiver_ids : undefined;
    state.pinned = true;
    state.compareThree = three;
    renderSelected(before);
  };
}
$("close-modal").onclick = () => $("modal").close();
$("modal").onclick = (e) => {
  if (e.target === $("modal") && e.offsetX < 0) $("modal").close();
};
$("help").onclick = methodology;
$("methodology").onclick = methodology;
$("nav-recordings").onclick = recordings;
$("nav-sources").onclick = sources;
$("nav-map").onclick = () => {
  $("map").scrollIntoView({ behavior: "smooth", block: "center" });
};
$("reset").onclick = () => resetFilters();
$("back-recording").onclick = () => resetFilters("replay");
$("apply").onclick = () => {
  if (state.mode === "live") $("follow-live").checked = false;
  digital.draw();
  clearSelection();
  state.pinned = false;
  state.manualClose = false;
  refresh(true);
};
function chooseBand(band) {
  $("band").value = band;
  state.overview = band === "all" && state.mode === "live" && worldwide();
  clearSelection();
  state.pinned = false;
  state.manualClose = false;
  drawPool();
  digital.draw();
  drawSchedule();
  refresh(true);
}
$("band").onchange = () => chooseBand($("band").value);
$("all-bands").onclick = () => chooseBand("all");
function worldView() {
  map.fitBounds([[-56,-179],[73,179]], { animate: false, padding: [1, 1] });
}
$("world-network").onclick = () => {
  state.overview = true;
  state.regionKey = null;
  if (state.selected) renderSelected();
  worldView();
};
$("show-pool").onchange = drawPool;
$("show-directory").onchange = drawPool;
for (const input of [$("start"), $("end")]) {
  input.oninput = () => {
    if (state.mode !== "live") return;
    $("follow-live").checked = false;
    digital.draw();
    renderHistory();
  };
}
$("mode-replay").onclick = () => setMode("replay");
$("mode-live").onclick = () => setMode("live");
$("replay").onclick = () => runJob(false);
$("test-sync").onclick = () => runJob(true);
$("fit").onclick = () => {
  if (state.mode !== "live" || receptionOn()) return fitNetwork();
  const solutions = (state.data?.signals || []).map(positionObservation).filter(Boolean).flatMap(e => e.estimate ? [e.estimate] : e.hypotheses);
  if (solutions.length) fitContours(solutions);
  else notify("No measured transmitter region on this band in the selected interval.");
};
function updateReceptionControls() {
  const hidden = state.mode === "live" && !receptionOn();
  for (const label of document.querySelectorAll(".reception-key, #show-pool, #show-directory, #directory-view")) {
    (label.closest("label") || label).hidden = hidden;
  }
  $("fit").lastChild.textContent = hidden ? "Fit transmitter positions" : "Fit receiver network";
}
$("show-reception").onchange = () => {
  updateReceptionControls();
  drawPool();
  drawRoster();
  digital.refresh();
  renderSelected();
};
setInterval(
  () => {
    $("clock").textContent = new Date(nowUtc()).toISOString().slice(11, 19) + " UTC";
    updateCountdown();
    drawSchedule();
  },
  1000,
);
async function boot() {
  icons();
  try {
    state.info = await api("/api/info");
    if (state.info.digital_reports) {
      const select = $("band");
      const known = new Set([...select.options].map(o => o.value));
      for (const [b] of state.info.band_ranges_mhz) if (!known.has(b)) select.add(new Option(b, b));
      select.add(new Option("Other reported bands", "other"));
    }
    $("dataset-count").textContent = state.info.datasets.length;
    const params = new URLSearchParams(location.search);
    const live = state.info.live_enabled && !params.has("replay");
    const requestedStation = live ? params.get("station")?.trim().slice(0,32).toUpperCase() : null;
    resetFilters(live ? "live" : "replay", params.has("field") || params.has("receiver") || params.get("view") === "activity", requestedStation, live ? params.get("band") : "all");
    await loadBeacons();
    if (live && params.has("receiver")) {
      const marker = [...pool.getLayers(), ...directoryOnly.getLayers()].find(l => l.options.receiverId === params.get("receiver"));
      if (marker) {
        map.setView(marker.getLatLng(), 8, {animate: false});
        marker.openPopup();
      } else notify("No mapped directory position is available for this receiver.");
    } else if (live && params.has("field")) await digital.focus(params.get("field").toUpperCase());
  } catch (err) {
    $("api-status").textContent = "API unavailable";
    notify(err.message);
  }
}
boot();
setInterval(() => { if (state.mode === "live" && !document.hidden) loadBeacons(); }, 30 * 60 * 1000);

function reviewCapture(id) {
  openModal(
    "Verify the captured emission",
    `<p>Capture <code>${esc(id)}</code> has been saved but not localized. An approved frequency does not prove that the recorded signal belongs to a beacon or participating station.</p><p>After RF verification, document how this capture is linked to an eligible emission. Its callsign remains unknown unless separately verified.</p><label for="review-url">Evidence URL for this emission / capture</label><input id="review-url" type="url"><label for="review-note">How the link to an eligible emission was verified</label><input id="review-note"><label for="review-token">Administrator token</label><input id="review-token" type="password" autocomplete="off"><button id="review-submit" class="primary-button">Save evidence and process</button><p id="review-error" role="alert"></p>`,
  );
  $("review-submit").onclick = async () => {
    try {
      const job = await api("/api/review", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer " + $("review-token").value,
        },
        body: JSON.stringify({
          capture_id: id,
          evidence: {
            url: $("review-url").value,
            note: $("review-note").value,
          },
          identity: null,
        }),
      });
      $("review-token").value = "";
      $("modal").close();
      setBusy(true);
      await waitJob(job.job_id);
      state.manualClose = false;
      await refresh(true);
      fitNetwork();
      notify("Evidence saved; TDoA processing finished.");
    } catch (err) {
      if ($("review-error")) $("review-error").textContent = err.message;
      notify(err.message);
    } finally {
      setBusy(false);
    }
  };
}

// Refresh data already being acquired, never open receiver connections from a timer.
setInterval(() => {
  if (state.mode === "live" && !document.hidden && !state.busy && state.info) {
    if ($("follow-live").checked) liveInterval();
    refresh(false);
    digital.refresh();
  }
}, 8000);
