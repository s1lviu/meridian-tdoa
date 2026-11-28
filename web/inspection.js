"use strict";

let inspectionRequest = 0, returnView = null, previousScope = "", previousIds = new Set();
const newIds = new Set();
const panel = () => document.querySelector(".panel");
const layout = () => document.querySelector(".layout");

function highlight(element) {
  if (!reducedMotion()) element.animate([{ opacity: 0.45 }, { opacity: 1 }], { duration: 180, easing: "ease-out" });
}
function recordArrivals(data, scope, profile) {
  const ids = new Set([...(data.emissions || []), ...(data.unlocated || [])]
    .filter(e => profile === "all" || e.profile === profile).map(e => e.id));
  const before = newIds.size;
  if (state.selected && scope === previousScope) {
    for (const id of ids) if (!previousIds.has(id)) newIds.add(id);
  } else newIds.clear();
  previousScope = scope;
  previousIds = ids;
  const button = $("new-observations");
  button.hidden = !newIds.size;
  button.textContent = "New observations · " + newIds.size;
  if (newIds.size > before) highlight(button);
}

function rememberOverview() {
  if (returnView) return;
  const active = document.activeElement;
  returnView = { center: map.getCenter(), zoom: map.getZoom(), focus: active, id: active?.dataset.id,
    group: active?.dataset.group, kind: active?.dataset.kind, scroll: window.scrollY, panel: panel().scrollTop,
    list: active?.closest(".list"), offsets: Array.from(document.querySelectorAll(".panel .list"), el => [el, el.scrollTop]) };
}
function closeInspection() {
  ++inspectionRequest;
  const saved = returnView;
  returnView = null;
  state.selected = null;
  layout().classList.remove("inspecting", "sheet-expanded");
  $("inspector").hidden = true;
  $("inspector").replaceChildren();
  newIds.clear();
  $("new-observations").hidden = true;
  map.invalidateSize({ pan: false });
  if (saved) map.setView(saved.center, saved.zoom, { animate: false });
  drawLists($("profile").value, saved?.list);
  updateOverview();
  drawOverview();
  if (!saved) return;
  for (const [list, top] of saved.offsets) list.scrollTop = top;
  panel().scrollTop = saved.panel;
  window.scrollTo(0, saved.scroll);
  const buttons = Array.from(document.querySelectorAll(".list button, .overview-marker"));
  const original = saved.focus?.isConnected ? saved.focus : buttons.find(b =>
    (saved.id && b.dataset.id === saved.id && b.dataset.kind === saved.kind) || (saved.group && b.dataset.group === saved.group));
  (original && original !== document.body ? original : $("map")).focus({ preventScroll: true });
}

function showInspection(selection, html) {
  rememberOverview();
  state.selected = selection;
  layout().classList.add("inspecting");
  $("map-key").open = false;
  const el = $("inspector");
  el.hidden = false;
  el.innerHTML = '<div class="inspection-nav"><button id="inspector-close"><i data-lucide="arrow-left"></i> Back to results</button>' +
    '<button id="sheet-expand" aria-expanded="false">Expand details</button></div>' + html;
  $("inspector-close").onclick = closeInspection;
  $("sheet-expand").onclick = () => {
    const expanded = layout().classList.toggle("sheet-expanded");
    $("sheet-expand").textContent = expanded ? "Compact details" : "Expand details";
    $("sheet-expand").setAttribute("aria-expanded", expanded);
    map.invalidateSize({ pan: false });
  };
  layout().classList.remove("sheet-expanded");
  panel().scrollTop = 0;
  el.scrollTop = 0;
  if (matchMedia("(max-width: 860px)").matches) window.scrollTo(0, 0);
  map.invalidateSize({ pan: false });
  if (selection.areas?.length) fitEvidence(selection.areas);
  else if (selection.receivers?.length) fitEvidence([], selection.receivers);
  drawOverview();
  icons();
  $("inspector-close").focus({ preventScroll: true });
  highlight(map.getPane("evidence"));
  highlight(el);
}

function selectGroup(title, rows, bounds) {
  ++inspectionRequest;
  const selection = { group: true, bounds };
  showInspection(selection, '<h3>' + esc(title) + '</h3><p class="sub">' + rows.length +
    ' results</p><ol id="group-list" class="list"></ol><button id="group-more" class="more">Show more results</button>');
  let limit = 0;
  const more = () => {
    limit += resultPage;
    renderRows($("group-list"), rows.slice(0, limit));
    $("group-more").hidden = limit >= rows.length;
  };
  $("group-more").onclick = more;
  more();
}

function viewActions(selection) {
  const regions = selection.areas?.length, rx = selection.receivers?.length;
  if (!regions && !rx) return "";
  return '<div class="view-actions">' +
    (regions ? '<button id="fit-regions">' + (regions > 1 ? "Show all regions" : "Show region") + '</button>' : "") +
    (rx ? '<button id="fit-receivers">Show receiver extent</button>' : "") +
    '<button id="open-globe"><i data-lucide="globe"></i> Globe view</button></div>';
}
function bindViewActions(selection) {
  if ($("fit-regions")) $("fit-regions").onclick = () => fitEvidence(selection.areas);
  if ($("fit-receivers")) $("fit-receivers").onclick = () => {
    $("show-receivers").checked = true; drawSelectionLayers(); fitEvidence([], selection.receivers);
  };
  if ($("open-globe")) $("open-globe").onclick = async () => {
    try {
      const [module, world] = await Promise.all([import("./globe.js"), loadWorld()]);
      if (state.selected === selection) await module.openGlobe(selection, world, () => {
        if (state.selected === selection) fitEvidence(selection.areas?.length ? selection.areas : [], selection.areas?.length ? [] : selection.receivers);
      });
    } catch { notify("Globe view unavailable. The flat map is available."); }
  };
}

async function selectTrack(key) {
  const request = ++inspectionRequest;
  try {
    const d = await api("/api/tracks/" + encodeURIComponent(key));
    if (request !== inspectionRequest) return;
    const t = d.track, el = d.ellipse_95, area = trackArea(d);
    const selection = { track: key, areas: [area], title: trackName(t) };
    const obs = d.observations.map(o => '<div class="rx-row"><span class="rx-dot ' + (o.estimate ? "used" : "heard") +
      '"></span><button class="link" data-id="' + esc(o.id) + '">' + esc(utc(o.start_utc)) + '</button><span>' +
      (o.estimate ? "±" + km(o.estimate.radius_km) : esc(o.status)) + '</span></div>').join("");
    showInspection(selection, `<h3>${esc(selection.title)}</h3><p class="sub">${esc(PROFILE[t.profile] || t.profile || "")} · ${esc(t.band || "")} · ${freq(t.frequency_hz)}</p>
      <p><span class="status-pill located">Located emitter</span></p><dl><dt>Position</dt><dd>${positionText(t.center, area.grid)}</dd>
      <dt>95% ellipse</dt><dd>${km(el.major_km)} × ${km(el.minor_km)}, axis ${fmt(el.azimuth_deg)}°</dd>
      <dt>Fused fixes</dt><dd>${t.fixes} (gate rejected ${t.rejected})</dd><dt>Last observed</dt><dd>${esc(utc(t.last_utc))}</dd>
      <dt>First observed</dt><dd>${esc(utc(t.first_utc))}</dd></dl>
      ${area.grid ? '<p class="note">95% model region, rounded outward to locator boundaries.</p>' : ""}${viewActions(selection)}
      <details open><summary>Contributing observations</summary>${obs}</details>
      <p class="note">Covariance intersection accounts for shared ionospheric and antenna errors across fixes.</p>`);
    for (const b of $("inspector").querySelectorAll(".link")) b.onclick = () => selectObservation(b.dataset.id);
    bindViewActions(selection);
  } catch (err) { if (request === inspectionRequest) notify(err.message); }
}

async function selectObservation(id) {
  const request = ++inspectionRequest;
  try {
    const e = await api("/api/emissions/" + encodeURIComponent(id));
    if (request !== inspectionRequest) return;
    const summary = state.data?.emissions.find(o => o.id === id);
    const areas = observationAreas(e, summary?.profile);
    const selection = { observation: id, title: e.identity?.callsign || e.title, areas, receivers: e.receivers };
    const ests = e.estimate ? [e.estimate] : e.hypotheses || [];
    const status = e.estimate ? "located" : ests.length || e.detected ? "undetermined" : "none";
    const ft8 = e.reference?.ft8;
    const rx = e.receivers.map(r => `<div class="rx-row"><span class="rx-dot ${r.used ? "used" : r.confirmed ? "heard" : ""}"></span><span>${esc(r.label || r.id)}</span><span class="note">${r.used ? "used" : r.confirmed ? "timed" : esc(r.reason || "not confirmed")}</span></div>`).join("");
    const pairs = e.pairs.slice(0, 40).map(p => `<div class="pair-row"><span>${esc(p.a.slice(0, 8))}→${esc(p.b.slice(0, 8))}</span><span>${us(p.delay_us)} ± ${us(p.sigma_us)}</span><span class="${p.accepted ? "good" : "bad"}">${p.accepted ? "accepted" : "rejected"}</span></div>`).join("");
    showInspection(selection, `<h3>${esc(selection.title)}</h3><p class="sub">${esc(utc(e.start_utc))} · ${freq(e.frequency_hz)}</p>
      <p><span class="status-pill ${!e.estimate && areas.length ? "ambiguous" : status}">${status === "located" ? "Located" : status === "undetermined" ? "Position undetermined" : "No detection"}</span></p>
      ${!e.estimate && areas.length ? '<div class="candidate-choices" aria-label="Possible regions">' + areas.map((a, i) => '<button class="region-jump" data-region="' + i + '">Region ' + a.name + '</button>').join("") + '</div>' : ""}
      ${viewActions(selection)}
      ${e.reason ? '<p class="note">' + esc(e.reason) + '</p>' : ""}
      ${areas[0]?.grid ? '<p class="note">95% model region, rounded outward to locator boundaries.</p>' : ""}
      ${ests.map((s, i) => `<dl><dt>${e.estimate ? "Estimate" : "Region " + areas[i].name}</dt><dd>${positionText(s.center, areas[i].grid)}</dd><dt>95% region</dt><dd>${km(s.radius_km)} max radius</dd><dt>Residual</dt><dd>${us(s.residual_us)} rms${s.scale ? " · scale " + fmt(s.scale, 2) : ""}</dd></dl>`).join("")}
      ${ft8 ? `<dl><dt>Message</dt><dd>${esc(ft8.message)}</dd><dt>Heard by</dt><dd>${ft8.receivers.filter(r => r.decoded).length} decoded · ${ft8.receivers.filter(r => r.arrival_s != null).length} timed</dd></dl>` : ""}
      <details><summary>Receivers · ${e.receivers.length}</summary>${rx}${e.excluded?.length ? '<p class="note">' + e.excluded.length + ' receiver(s) excluded by the RF verifier.</p>' : ""}</details>
      <details><summary>Pair delays (B − A)</summary>${pairs || '<p class="note">No pair of timed receivers.</p>'}</details>
      <div id="evaluation"></div><p class="note">${esc(e.provenance?.note || "")}</p>`);
    for (const b of $("inspector").querySelectorAll(".region-jump")) b.onclick = () => fitEvidence([areas[Number(b.dataset.region)]]);
    bindViewActions(selection);
    try {
      const ev = await api("/api/emissions/" + encodeURIComponent(id) + "/evaluation");
      if (request !== inspectionRequest) return;
      $("evaluation").innerHTML = `<details><summary>Post-hoc check</summary><p class="note">Reference: ${esc(ev.reference_source)}${ev.grid ? " · locator " + esc(ev.grid) : ""}. Used only for evaluation.</p>` +
        ev.solutions.map(s => `<div class="pair-row"><span>solution</span><span>${km(s.error_km)}${s.error_range_km ? " (" + km(s.error_range_km[0]) + "–" + km(s.error_range_km[1]) + ")" : ""}</span><span>±${km(s.model_radius_km)}</span></div>`).join("") + "</details>";
    } catch { /* Some observations have no hold-out reference. */ }
  } catch (err) { if (request === inspectionRequest) notify(err.message); }
}
