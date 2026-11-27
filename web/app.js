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
const frequency = (f) => `${fmt(f / 1000, f % 1000 ? 2 : 0)} kHz`;
const utc = (t) => new Date(t).toISOString().slice(11, 23);
const utcDate = (t) =>
  new Date(t).toLocaleDateString("en-GB", {
    timeZone: "UTC",
    day: "2-digit",
    month: "short",
    year: "numeric",
  });
const name = (e) => e.identity?.callsign || "unknown callsign";
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
};
const map = L.map("map", {
  zoomControl: true,
  minZoom: 2,
  maxZoom: 14,
  worldCopyJump: true,
}).setView([49.5, 6.5], 5);
L.control
  .scale({ imperial: false, position: "bottomleft", maxWidth: 80 })
  .addTo(map);
map.createPane("fallback");
map.getPane("fallback").style.zIndex = "100";
map.createPane("countryLabels");
map.getPane("countryLabels").style.zIndex = "210";
map.getPane("countryLabels").style.pointerEvents = "none";
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
      for (const f of data.features) {
        const p = f.properties;
        if (Number.isFinite(p.LABEL_Y) && Number.isFinite(p.LABEL_X))
          L.marker([p.LABEL_Y, p.LABEL_X], {
            pane: "countryLabels",
            interactive: false,
            keyboard: false,
            icon: L.divIcon({
              html: esc(p.NAME_EN || p.NAME),
              className: "country-label",
              iconSize: [100, 16],
              iconAnchor: [50, 8],
            }),
          }).addTo(map);
      }
    }
  })
  .catch(() => {});
map.attributionControl.addAttribution('<a href="https://www.naturalearthdata.com/">Natural Earth</a> · local vector basemap');
const regions = L.layerGroup().addTo(map),
  network = L.layerGroup().addTo(map);
let refreshTimer;
map.on("moveend", () => {
  const c = map.getCenter();
  $("map-position").textContent =
    `${Math.abs(c.lat).toFixed(2)}°${c.lat >= 0 ? "N" : "S"} / ${Math.abs(c.lng).toFixed(2)}°${c.lng >= 0 ? "E" : "W"} / ${map.getZoom()}×`;
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => refresh(false), 180);
  if (state.selected) renderInspector(state.selected);
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
    renderRegions(data.emissions);
    renderCoverage(data.monitoring);
    const available = [...data.emissions, ...data.unlocated];
    if ((selectFirst || !state.selected) && !state.manualClose) {
      if (available.length) await selectEmission(available[0].id);
      else clearSelection();
    }
    $("empty-map").hidden = !!state.selected || available.length > 0;
    if (!state.selected) {
      drawSpectrum([]);
      const h = $("empty-map").querySelector("h2"),
        p = $("empty-map").querySelector("p");
      if (data.monitoring.segments.some((s) => s.status === "no_detection")) {
        h.textContent = "No emission detected";
        p.textContent =
          "The detector found no emission in the recorded channels. Gaps remain unmonitored.";
      } else if (data.total_matching > 0) {
        h.textContent = "No region in this view";
        p.textContent =
          "Results exist outside the visible map. Pan or zoom to see them.";
      } else {
        h.textContent = "Unmonitored interval";
        p.textContent =
          "No recordings match these filters. Missing data does not mean there were no emissions.";
      }
    }
    icons();
  } catch (err) {
    if (serial !== state.request) return;
    $("filter-error").textContent = err.message;
    $("api-status").textContent = "Data unavailable";
    $("api-dot").style.background = "var(--amber)";
  }
}
function renderList(data) {
  $("emission-count").textContent = data.emissions.length;
  const card = (e, unknown = false) =>
    `<button class="emission-card ${unknown ? "unlocated-card" : ""} ${state.selected?.id === e.id ? "selected" : ""}" data-emission="${esc(e.id)}"><div class="emission-card-top"><strong>${esc(name(e))}</strong><span class="micro-tag">${unknown ? "UNDETERMINED" : e.hypotheses?.length ? "AMBIGUOUS" : "TDoA"}</span></div><div class="emission-frequency">${frequency(e.frequency_hz)}</div><div class="emission-meta"><i data-lucide="clock-3"></i>${utc(e.start_utc).slice(0, 8)} UTC <span>·</span> ${e.receivers.filter((r) => r.confirmed).length} RX</div></button>`;
  $("emissions").innerHTML =
    data.emissions.map((e) => card(e)).join("") ||
    '<div class="empty-list">No estimated region in the current view.</div>';
  $("unlocated").innerHTML = data.unlocated.length
    ? `<div class="unlocated-title">Unlocated · cannot be filtered geographically</div>${data.unlocated.map((e) => card(e, true)).join("")}`
    : "";
  document.querySelectorAll("[data-emission]").forEach(
    (b) =>
      (b.onclick = () => {
        state.manualClose = false;
        selectEmission(b.dataset.emission);
      }),
  );
}
function renderRegions(emissions) {
  regions.clearLayers();
  for (const e of emissions) {
    const solutions = e.estimate ? [e.estimate] : e.hypotheses || [];
    solutions.forEach((est, index) => {
      const ambiguous = !e.estimate;
      const polygon = L.polygon(
        est.polygon.map((p) => [p.lat, p.lon]),
        {
          color: ambiguous ? "#e7b879" : "#c0ef85",
          weight: 1.6,
          fillColor: ambiguous ? "#e7b879" : "#b2df7a",
          fillOpacity: 0.16,
          dashArray: "5 4",
        },
      ).addTo(regions);
      polygon.on("click", () => selectEmission(e.id));
      if (ambiguous) {
        polygon.bindTooltip("Hypothesis " + (index + 1) + " · " + esc(name(e)), {
          permanent: true,
          direction: "bottom",
          offset: [0, 12],
          className: "tx-label ambiguous-label",
        });
      } else {
        const p = est.center;
        L.marker([p.lat, p.lon], {
          icon: L.divIcon({
            html: '<div class="tx-icon"></div>',
            className: "",
            iconSize: [17, 17],
            iconAnchor: [8, 8],
          }),
        })
          .addTo(regions)
          .bindTooltip(esc(name(e)), {
            permanent: true,
            direction: "bottom",
            offset: [0, 15],
            className: "tx-label",
          })
          .on("click", () => selectEmission(e.id));
      }
    });
  }
}
async function selectEmission(id) {
  const request=++state.selectionRequest;
  try {
    const e = await api("/api/emissions/" + encodeURIComponent(id));
    if(request!==state.selectionRequest)return;
    state.selected = e;
    $("empty-map").hidden = true;
    renderInspector(e);
    renderNetwork(e);
    drawSpectrum(e.spectrum, e.frequency_hz);
    if (state.data) renderList(state.data);
    icons();
  } catch (err) {
    notify(err.message);
  }
}
function clearSelection() {
  state.selectionRequest++;
  state.selected = null;
  $("inspector").hidden = true;
  network.clearLayers();
  drawSpectrum([]);
}
function renderNetwork(e) {
  network.clearLayers();
  for (const r of e.receivers) {
    if (!r.confirmed) continue;
    const p = r.position;
    const marker = L.marker([p.lat, p.lon], {
      icon: L.divIcon({
        html: `<div class="rx-icon ${r.used ? "" : "unused"}"></div>`,
        className: "",
        iconSize: [12, 12],
        iconAnchor: [6, 6],
      }),
    }).addTo(network);
    marker.bindTooltip(esc(r.label), {
      permanent: true,
      direction: "top",
      offset: [0, -10],
      className: "rx-label",
    });
    marker.on("click", () => receiverProof(r));
    if (e.estimate)
      L.polyline(
        [
          [p.lat, p.lon],
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
  $("inspector").hidden = false;
  const est = e.estimate,
    confirmed = e.receivers.filter((r) => r.confirmed),
    used = e.receivers.filter((r) => r.used);
  const b = map.getBounds();
  $("inspector").innerHTML =
    `<div class="inspector-head"><div class="inspector-heading-row"><span>SELECTED EMISSION</span><button id="close-inspector" aria-label="Close details">×</button></div><div class="signal-name"><i data-lucide="radio"></i>${esc(name(e))}</div><div class="identity-note">${e.identity ? "Identity documented for this recording" : "No verifiable identity associated"}</div></div><div class="inspector-values"><div><div class="metric-label">FREQUENCY</div><div class="metric-value">${frequency(e.frequency_hz)}</div></div><div><div class="metric-label">CAPTURE UTC</div><div class="metric-value">${utc(e.start_utc).slice(0, 8)}</div></div><div><div class="metric-label">UNCERTAINTY / MODEL</div><div class="metric-value green">${est ? "≤ " + fmt(est.radius_km) + " km" : e.hypotheses?.length ? e.hypotheses.length + " hypotheses" : "—"}</div><div class="metric-foot">${est ? "Maximum contour radius" : "Position undetermined"}</div></div><div><div class="metric-label">RECEIVERS USED</div><div class="metric-value">${used.length} <span class="dim">/ ${confirmed.length}</span></div><div class="metric-foot">${fmt((new Date(e.end_utc) - new Date(e.start_utc)) / 1000, 1)} s simultaneous</div></div></div><div class="quality-strip ${est ? "" : "warning"}"><i data-lucide="${est ? "circle-check" : "triangle-alert"}"></i><span>${est ? "GNSS + correlation + geometry validated" : e.hypotheses?.length ? "Position undetermined · two compatible global solutions" : `Position undetermined · ${esc(e.reason)}`}</span></div><div class="rx-section"><div class="rx-heading"><span>All confirmed receptions</span><span>${confirmed.length} RX</span></div>${confirmed.map((r) => `<div class="rx-row"><span class="mini-rx ${r.used ? "" : "unused"}"></span><button data-rx="${esc(r.id)}">${esc(r.label)}</button>${b.contains([r.position.lat, r.position.lon]) ? "" : '<span class="outside">outside view</span>'}<span class="rx-state ${r.used ? "" : "unused"}">${r.used ? "USED" : !r.synchronized ? "NO GNSS" : "EXCLUDED"}</span></div>`).join("") || '<p class="caption muted">No confirmed correlated reception.</p>'}</div><div class="inspector-footer"><button id="proof"><i data-lucide="file-check-2"></i>Evidence and correlations</button><button id="export"><i data-lucide="download"></i>JSON</button></div>`;
  $("close-inspector").onclick = () => {
    state.manualClose = true;
    clearSelection();
    refresh();
  };
  $("proof").onclick = () => showProof(e);
  $("export").onclick = () => {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(e, null, 2)], { type: "application/json" }),
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
    ? "No recordings cover this interval. No conclusion about the absence of emissions is possible."
    : `${m.gaps.length ? "Parts of this interval were not monitored. " : ""}Only the recorded channel is covered, not the entire band or map.`;
  $("capture-window").textContent = m.segments.length
    ? `${utc(m.segments[0].start).slice(0, 8)} — ${utc(m.segments[0].end).slice(0, 8)} UTC · ${frequency(m.segments[0].frequency_hz)}`
    : "—";
}
function fitNetwork() {
  const e = state.selected;
  if (!e) return;
  let points = e.receivers
    .filter((r) => r.confirmed)
    .map((r) => [r.position.lat, r.position.lon]);
  if (e.estimate) points.push(...e.estimate.polygon.map((p) => [p.lat, p.lon]));
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
    `<h3>Reception and synchronization</h3><p>${r.confirmed ? "RF correlation confirmed." : "Reception of the same emission was not confirmed."} ${r.used ? "Receiver used in the calculation." : "Receiver excluded from the calculation."}</p><p>${esc(r.reason || "Continuous GNSS timestamps; solution age ≤ 2 s in the analysed segment.")}</p><table><tr><td>Published position</td><td>${fmt(r.position.lat, 6)}°, ${fmt(r.position.lon, 6)}°</td></tr><tr><td>Sample rate measured from GNSS</td><td>${fmt(r.sample_rate_hz, 3)} Hz</td></tr><tr><td>Maximum GNSS age</td><td>${r.gps_age_max_s === 255 ? "No solution" : r.gps_age_max_s + " s"}</td></tr><tr><td>Samples in the common interval</td><td>${r.samples}</td></tr></table><h3>Position provenance</h3><p>${esc(r.position_evidence.note)}</p><p>${link(r.position_evidence.url, "Coordinate source ↗")}</p><h3>Recording integrity</h3><p class="proof"><code>${esc(r.sha256)}</code></p><p>An archived receiver is not claimed to be currently available or authorized for live capture.</p>`,
  );
}
function showProof(e) {
  openModal(
    "Evidence · " + name(e),
    `<span class="badge ${e.fault_injected ? "test" : e.mode === "live" ? "live" : "replay"}">${e.fault_injected ? "RECORDING TEST" : e.mode === "live" ? "LIVE" : "REAL RECORDING"}</span><p>${esc(e.provenance.note)}</p><p>${link(e.provenance.url, "Recording source ↗")}</p><h3>Identity</h3><p>${e.identity ? esc(e.identity.evidence.note) : "unknown callsign"}</p>${e.identity ? link(e.identity.evidence.url, "Evidence linking identity to this emission ↗") : ""}<h3>Measured delays</h3><table><thead><tr><th>Pair</th><th>Δt (µs)</th><th>Correlation</th><th>Windows</th></tr></thead><tbody>${e.pairs.map((p) => `<tr><td>${esc(p.a)} → ${esc(p.b)}</td><td>${fmt(p.delay_us, 2)}</td><td>${fmt(p.coherence, 3)}</td><td>${p.windows} ${p.accepted ? "✓" : "×"}</td></tr>`).join("")}</tbody></table><p>Δt = arrival B − arrival A. Normalized correlation is not a probability. Unsynchronized receivers may have correlated reception but are excluded from localization.</p><h3>Uncertainty and limitations</h3><p>${esc(e.estimate?.uncertainty_note || e.reason)}</p>${e.hypotheses?.length ? '<p>These contours represent alternative hypotheses. The transmitter reference is never used to select one.</p><button class="primary-button" id="all-hypotheses">Show all hypotheses on the map</button>' : ""}<p>Published receiver coordinates do not establish exact antenna offsets. Spherical propagation is an approximation; the result does not guarantee accuracy.</p><h3>Separate evaluation</h3><p>The known transmitter position is not used by the solver, receiver selection or search domain.</p><button class="primary-button" id="show-evaluation">View real-data evaluation</button>`,
  );
  $("show-evaluation").onclick = evaluation;
  if ($("all-hypotheses"))
    $("all-hypotheses").onclick = () => {
      $("modal").close();
      map.fitBounds(
        e.hypotheses.flatMap((h) => h.polygon.map((p) => [p.lat, p.lon])),
        { padding: [60, 60] },
      );
    };
}
async function evaluation() {
  try {
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
    `<h3>01 / Independent capture</h3><p>At least three independent SDRs record simultaneous IQ with GNSS timestamps. Live capture checks API authorization, validity, bands, antenna location and receiver status. A public listing alone does not establish permission.</p><h3>02 / Same signal, common timebase</h3><p>SHA-256, Kiwi WAV blocks, GNSS continuity and age are checked. The first settling second is excluded. Streams are resampled onto a common interval; complex correlation is measured in 2-second windows. Ambiguous tones and insufficient correlations are rejected.</p><h3>03 / TDoA only</h3><p>Position is derived from time differences, with geometry and alternative solutions checked. RSSI, callsigns and known transmitter positions are not localization inputs. Contours show model-dependent uncertainty; statistical coverage is not calibrated.</p><h3>04 / Explicit limits</h3><p>Visible receivers do not establish geographic coverage. “No emission detected” only applies to the recorded channel and interval under the documented detector thresholds. Other intervals remain unmonitored. DCF77 is a public time reference used for commissioning, not an amateur station.</p><h3>Primary documentation</h3><p>${link("https://github.com/jks-prv/kiwiclient", "KiwiClient · capture and IQ format ↗")}<br>${link("https://github.com/hcab14/TDoA", "TDoA · original recording and receiver coordinates ↗")}<br>${link("https://www.ptb.de/cms/fileadmin/internet/publikationen/masstaebe/Hefte_Komplett_PDF/mst01.pdf", "PTB · DCF77 reference ↗")}</p><button class="primary-button" id="method-eval">Evaluation report</button>`,
  );
  $("method-eval").onclick = evaluation;
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
        resetFilters();
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
    openModal("Public SDR receivers", `
      <span class="badge ${state.info.live_enabled ? "live" : "replay"}">${state.info.live_enabled ? "CAPTURE ON DEMAND" : "CAPTURE DISABLED"}</span>
      <p>Catalog: <strong>${d.total ?? 0}</strong>receivers · published API allowance: <strong>${d.api_allowed ?? 0}</strong> ·TDoA candidates: <strong>${d.candidates ?? 0}</strong>.</p>
      <p>${d.fetched_at ? "Updated at " + esc(d.fetched_at) + (d.stale ? " · STALE METADATA" : "") : "The directory is not available yet."} ${esc(d.note)}</p>
      <p>A candidate is not a confirmed reception. Selected receivers are checked directly before each capture; GNSS and correlation are then verified in the IQ data.</p>
      <form id="catalog-form"><label for="catalog-search">Search receiver / antenna</label><input id="catalog-search" value="${esc(catalogSearch)}" maxlength="100"><label class="catalog-check"><input type="checkbox" id="catalog-eligible" ${catalogEligible ? "checked" : ""}> TDoA candidates only</label><button class="primary-button">Search directory</button></form>
      <p id="catalog-selection">Selected: ${selectedSources.size}/8</p><p id="catalog-error" role="alert"></p>
      <div class="catalog-list">${(d.receivers || []).map(({receiver: r, exclusion}) => `<div class="dataset-card"><label class="catalog-check"><input type="checkbox" data-source-id="${esc(r.id)}" ${selectedSources.has(r.id) ? "checked" : ""} ${exclusion ? "disabled" : ""}><strong>${esc(r.name)}</strong></label><p>${esc(r.id)} · ${exclusion ? esc(exclusion) : "Candidate · IQ not verified"}<br>API: ${r.ext_api ?? "unpublished"}· channels ${r.users}/${r.users_max} · GNSS ${r.gps_good}<br>${esc(r.antenna)}<br>Published position: ${r.gps ? r.gps.map(n => fmt(n, 5)).join("°, ") + "°" : "unknown"}</p>${link(r.url, "Operator website ↗")}</div>`).join("")}</div>
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
function resetFilters() {
  $("toast").hidden = true;
  state.mode = "replay";
  state.scenario = "normal";
  state.manualClose = false;
  $("band").value = "all";
  const d =
    state.info.datasets.find((d) => d.id === state.datasetId) ||
    state.info.datasets[0];
  $("start").value = (d?.capture_utc || "2020-08-13T06:52:20Z").slice(0, 19);
  $("end").value = new Date(
    new Date(d?.capture_utc || "2020-08-13T06:52:20Z").getTime() + 35000,
  )
    .toISOString()
    .slice(0, 19);
  clearSelection();
  updateModeLabels();
  map.setView([49.5, 6.5], 5);
  refresh(true).then(fitNetwork);
}
function setMode(mode) {
  if (mode === state.mode) return;
  if (mode === "replay") {
    resetFilters();
    return;
  }
  state.mode = mode;
  state.scenario = "normal";
  state.manualClose = false;
  const now = new Date();
  $("start").value = new Date(+now - 3600000).toISOString().slice(0, 19);
  $("end").value = new Date(+now + 120000).toISOString().slice(0, 19);
  clearSelection();
  updateModeLabels();
  refresh(true);
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
$("reset").onclick = resetFilters;
$("back-recording").onclick = resetFilters;
$("apply").onclick = () => {
  if (state.mode === "live") $("follow-live").checked = false;
  clearSelection();
  state.manualClose = false;
  refresh(true);
};
$("band").onchange = () => {
  clearSelection();
  state.manualClose = false;
  refresh(true);
};
$("mode-replay").onclick = () => setMode("replay");
$("mode-live").onclick = () => setMode("live");
$("replay").onclick = () => runJob(false);
$("test-sync").onclick = () => runJob(true);
$("fit").onclick = fitNetwork;
setInterval(
  () =>
    ($("clock").textContent = new Date().toISOString().slice(11, 19) + " UTC"),
  1000,
);
async function boot() {
  icons();
  try {
    state.info = await api("/api/info");
    $("dataset-count").textContent = state.info.datasets.length;
    resetFilters();
    if (state.info.live_enabled && !new URLSearchParams(location.search).has("replay")) setMode("live");
  } catch (err) {
    $("api-status").textContent = "API unavailable";
    notify(err.message);
  }
}
boot();

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
    if ($("follow-live").checked) {
      const now=Date.now();
      $("start").value=new Date(now-3600000).toISOString().slice(0,19);
      $("end").value=new Date(now+120000).toISOString().slice(0,19);
    }
    refresh(false);
  }
}, 15000);
