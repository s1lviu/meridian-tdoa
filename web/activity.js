"use strict";

function digitalActivity(map) {
  map.createPane("digitalFields").style.zIndex = "205";
  const polygons = new Map();
  let latest = null, pending = null;
  function bounds(field) {
    const west = -180 + (field.charCodeAt(0) - 65) * 20;
    const south = -90 + (field.charCodeAt(1) - 65) * 10;
    return [[south,west],[south+10,west+20]];
  }
  function draw() {
    const enabled = state.mode === "live" && state.info?.digital_reports && receptionOn();
    $("digital-panel").hidden = !enabled;
    const visible = enabled && $("follow-live").checked;
    const selected = $("band").value;
    const modes = ["FT8", "WSPR"].filter(m => $("digital-" + m.toLowerCase()).checked);
    const ranges = state.info?.band_ranges_mhz || [];
    const bandIndex = b => ranges.findIndex(([name]) => name === b);
    function matchesBand(b) {
      if (selected === "all" || selected === b) return true;
      const low = ranges[bandIndex(b)]?.[1];
      return selected === "lf" && low < .3 || selected === "mf" && low >= .3 && low < 3 ||
        selected === "hf" && low >= 3 && low < 30;
    }
    const counts = new Map(), rows = new Map();
    let reports = 0, newest;
    for (const r of (visible && latest?.bands || [])) {
      if (!rows.has(r.band)) rows.set(r.band, {FT8:0, WSPR:0, latest:r.latest_utc});
      const row = rows.get(r.band);
      row[r.mode] = r.reports;
      if (row.latest < r.latest_utc) row.latest = r.latest_utc;
      if (!matchesBand(r.band) || !modes.includes(r.mode)) continue;
      reports += r.reports;
      if (!newest || r.latest_utc > newest) newest = r.latest_utc;
      for (const [field, n] of Object.entries(r.fields)) {
        if (!counts.has(field)) counts.set(field, {FT8:0, WSPR:0});
        counts.get(field)[r.mode] += n;
      }
    }
    for (const [field, polygon] of polygons) {
      if (!counts.has(field)) { polygon.remove(); polygons.delete(field); }
    }
    const mapped = [...counts.values()].map(c => c.FT8 + c.WSPR);
    const missing = reports - mapped.reduce((sum,n) => sum+n, 0);
    const peak = Math.max(1, ...mapped);
    for (const [field, c] of counts) {
      const strength = Math.log1p(c.FT8 + c.WSPR) / Math.log1p(peak);
      const color = strength < .33 ? "#526f91" : strength < .67 ? "#737db2" : "#a88ac8";
      const style = {color, weight:0.8, opacity:.65, fillColor:color, fillOpacity:.12 + strength * .30};
      let polygon = polygons.get(field);
      if (!polygon) {
        polygon = L.rectangle(bounds(field), {pane:"digitalFields", className:"digital-field", ...style})
          .addTo(map).bindTooltip("").bindPopup("",{className:"receiver-popup"});
        polygons.set(field,polygon);
      } else polygon.setStyle(style);
      const label = `${field} · ${modes.map(m => `${m} ${fmt(c[m],0)}`).join(" · ")}`;
      polygon.setTooltipContent(label);
      polygon.setPopupContent(`<strong>Reported transmitter field ${field}</strong><p>${label}</p><small>20° × 10° · reports in the live UTC window</small>`);
    }
    $("digital-legend").hidden = !counts.size;
    $("digital-legend").querySelector("span").textContent = `${modes.join(" + ")} · ${fmt(reports,0)} reports ▾`;
    const allBands = [...rows.keys()].sort((a,b) => bandIndex(a) - bandIndex(b));
    $("digital-rows").innerHTML = allBands.map(b => {
      const row = rows.get(b);
      return `<tr${selected===b ? ' class="band-selected"' : ""}><th scope="row"><button type="button" data-report-band="${esc(b)}" class="text-button">${esc(b)}</button></th><td>${fmt(row.FT8,0)}</td><td>${fmt(row.WSPR,0)}</td><td>${utc(row.latest).slice(0,8)}</td></tr>`;
    }).join("");
    if (!enabled) return;
    const feed = latest?.connected ? `Connected since ${utc(latest.connected_since_utc).slice(0,8)} UTC` : "Feed disconnected";
    $("digital-status").textContent = !visible ? "Follow the live UTC interval to see digital reports." :
      !latest ? "Waiting for the digital activity feed" :
      `${feed} · ${fmt(reports,0)} reports on selected bands and modes · ${counts.size} mapped fields` +
      (missing ? ` · ${fmt(missing,0)} reports without a field` : "") +
      (newest ? ` · latest ${utc(newest).slice(0,8)} UTC` : "") +
      ` · ${fmt(latest.reporters,0)} reporting stations across all bands and modes`;
  }
  async function refresh() {
    if (!state.info?.digital_reports || state.mode !== "live" || !receptionOn() || !$("follow-live").checked) { draw(); return; }
    if (!pending) pending = api("/api/activity").then(data => { latest = data; })
      .catch(() => { latest = null; }).finally(() => { pending = null; draw(); });
    await pending;
  }
  async function focus(field) {
    if (!/^[A-R]{2}$/.test(field) || !state.info.digital_reports) return;
    await refresh();
    map.fitBounds(bounds(field), {animate:false, maxZoom:6, padding:[20,20]});
    if (polygons.has(field)) polygons.get(field).openPopup();
    else notify(`No mapped reports in ${field} for the current live window.`);
  }
  $("digital-ft8").onchange = $("digital-wspr").onchange = draw;
  $("digital-legend").onclick = () => {
    $("digital-bands").open = true;
    $("digital-bands").querySelector("summary").focus();
    $("digital-bands").scrollIntoView({block:"center"});
  };
  $("digital-rows").onclick = e => {
    const b = e.target.closest("[data-report-band]")?.dataset.reportBand;
    if (b) chooseBand(b);
  };
  return {draw, refresh, focus};
}
