"use strict";

const overview = { tracks: [], ambiguous: [], singles: [], wide: [], signature: null, hits: [] };
const baseCopies = new Map();
let worldGeometry, coverageCanvas, coverageMask, coverageEdge, hitContext;
const nearLongitude = (lon, ref) => lon + 360 * Math.round((ref - lon) / 360);
const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

function region(points, center, grid, type, name) {
  const ring = displayRegion(points, center, grid);
  return { ring, bounds: L.latLngBounds(ring), anchor: grid ? locatorCell(center).center : [center.lat, center.lon], grid, type, name };
}
function trackArea({ track: t, ellipse_95: el }) {
  return region(ellipse(t.center, el.major_km, el.minor_km, el.azimuth_deg), t.center,
    gridDisplay(t.profile, t.callsign), "track", trackName(t));
}
function observationAreas(e, profile) {
  const grid = gridDisplay(e.reference?.profile || e.profile || profile, e.identity?.callsign);
  return (e.estimate ? [e.estimate] : e.hypotheses || []).map((s, i) =>
    region(polygon(s), s.center, grid, e.estimate ? "fix" : "hypothesis", e.estimate ? label(e) : candidateName(i)));
}
const candidateName = i => i < 26 ? String.fromCharCode(65 + i) : String(i + 1);
function visibleRing(area) {
  const lon = area.bounds.getCenter().lng;
  const shift = nearLongitude(lon, map.getCenter().lng) - lon;
  return shift ? area.ring.map(([lat, lng]) => [lat, lng + shift]) : area.ring;
}
function visiblePoint([lat, lon]) { return [lat, nearLongitude(lon, map.getCenter().lng)]; }

// Remove the largest uncovered longitude gap. Continuous uncertainty bounds may cross ±180°.
function fitEvidence(areas, receiverRows = []) {
  const bounds = areas.map(a => a.bounds).concat(receiverRows.map(r => {
    const point = [r.position.lat, r.position.lon];
    return L.latLngBounds([point]);
  }));
  if (!bounds.length) return;
  const intervals = [];
  let south = 90, north = -90;
  for (const b of bounds) {
    south = Math.min(south, b.getSouth()); north = Math.max(north, b.getNorth());
    const start = ((b.getWest() % 360) + 360) % 360;
    const end = start + Math.min(360, b.getEast() - b.getWest());
    intervals.push([start, Math.min(end, 360)]);
    if (end > 360) intervals.push([0, end - 360]);
  }
  intervals.sort((a, b) => a[0] - b[0]);
  let end = intervals[0][1], start = intervals[0][0], gap = 0;
  for (const [lo, hi] of intervals.slice(1)) {
    if (lo - end > gap) { gap = lo - end; start = lo; }
    end = Math.max(end, hi);
  }
  const wrapGap = intervals[0][0] + 360 - end;
  if (wrapGap > gap) { gap = wrapGap; start = intervals[0][0]; }
  const half = (360 - gap) / 2;
  const middle = nearLongitude(start + half, map.getCenter().lng);
  map.fitBounds([[south, middle - half], [north, middle + half]], { padding: [30, 38], maxZoom: 9, animate: false });
}

function drawLand() {
  if (!worldGeometry) return;
  const bounds = map.getBounds();
  const first = Math.floor((bounds.getWest() + 180) / 360), last = Math.floor((bounds.getEast() + 180) / 360);
  for (const [copy, layer] of baseCopies) if (copy < first || copy > last) { land.removeLayer(layer); baseCopies.delete(copy); }
  for (let copy = first; copy <= last; copy++) {
    if (baseCopies.has(copy)) continue;
    const layer = L.geoJSON(worldGeometry, { pane: "geography", interactive: false,
      coordsToLatLng: ([lon, lat]) => L.latLng(lat, lon + 360 * copy),
      style: { color: "#3d5260", weight: 0.65, fillColor: "#253743", fillOpacity: 1 },
    }).addTo(land);
    baseCopies.set(copy, layer);
  }
}

function updateOverview() {
  const footprints = new Map();
  for (const row of resultRows.ambiguous) {
    for (const area of observationAreas(row.raw)) {
      const key = JSON.stringify(area.ring);
      if (!footprints.has(key)) footprints.set(key, { area, rows: [] });
      footprints.get(key).rows.push(row);
    }
  }
  overview.singles = resultRows.single;
  const signature = JSON.stringify([
    resultRows.track.map(r => [r.id, r.raw.track.center, r.raw.ellipse_95, r.name, r.raw.track.profile, r.raw.track.last_utc, r.raw.track.fixes]),
    Array.from(footprints, ([ring, group]) => [ring, group.rows.map(r => [r.id, r.name])]),
  ]);
  if (signature === overview.signature) {
    if ($("show-observations").checked) drawSingles();
    return;
  }
  overview.signature = signature;
  overview.tracks = resultRows.track.map(row => ({ row, area: trackArea(row.raw) }));
  overview.ambiguous = [...footprints.values()];
  drawOverview();
}

// Paint all possible regions into a single opaque mask. Only its outer edge and one
// faint fill reach the map. Coincident observations cannot accumulate brightness.
function drawPossibleAreas() {
  const { x: w, y: h } = map.getSize();
  if (!w || !h) return;
  const ratio = Math.min(devicePixelRatio || 1, 2);
  const width = Math.ceil(w * ratio), height = Math.ceil(h * ratio);
  const canvases = [coverageMask, coverageEdge, coverageCanvas];
  const [mask, edge, paint] = canvases.map(canvas => {
    if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
    const ctx = canvas.getContext("2d");
    ctx.resetTransform(); ctx.clearRect(0, 0, width, height);
    return ctx;
  });
  coverageCanvas.style.width = w + "px"; coverageCanvas.style.height = h + "px";
  L.DomUtil.setPosition(coverageCanvas, map.containerPointToLayerPoint([0, 0]));
  overview.hits = [];
  mask.scale(ratio, ratio);
  mask.fillStyle = mask.strokeStyle = "#c79bff";
  mask.lineWidth = 2;
  for (const { area, rows } of overview.ambiguous) {
    const points = visibleRing(area).map(p => map.latLngToContainerPoint(p));
    const box = L.bounds(points);
    if (box.max.x < -4 || box.max.y < -4 || box.min.x > w + 4 || box.min.y > h + 4) continue;
    const path = new Path2D();
    path.moveTo(points[0].x, points[0].y);
    for (const p of points.slice(1)) path.lineTo(p.x, p.y);
    path.closePath();
    mask.fill(path);
    if (box.getSize().x < 3 || box.getSize().y < 3) mask.stroke(path);
    overview.hits.push({ path, box, rows });
  }
  if (!overview.hits.length) return;
  const offset = Math.ceil(ratio);
  for (const [dx, dy] of [[offset, 0], [-offset, 0], [0, offset], [0, -offset]]) edge.drawImage(coverageMask, dx, dy);
  edge.globalCompositeOperation = "destination-out";
  edge.drawImage(coverageMask, 0, 0);
  edge.globalCompositeOperation = "source-over";
  paint.globalAlpha = 0.075; paint.drawImage(coverageMask, 0, 0);
  paint.globalAlpha = 0.72; paint.drawImage(coverageEdge, 0, 0);
  paint.globalAlpha = 1;
}

function overviewClick({ containerPoint: p }) {
  if (state.selected) return;
  const matches = new Map();
  hitContext.lineWidth = 12;
  for (const { path, box, rows } of overview.hits) {
    if (p.x < box.min.x - 6 || p.x > box.max.x + 6 || p.y < box.min.y - 6 || p.y > box.max.y + 6) continue;
    if (hitContext.isPointInPath(path, p.x, p.y) || hitContext.isPointInStroke(path, p.x, p.y)) {
      for (const row of rows) matches.set(row.id, row);
    }
  }
  if (matches.size === 1) selectObservation(matches.keys().next().value);
  else if (matches.size) selectGroup("Ambiguous observations", [...matches.values()]);
}

function drawSingles() {
  fixes.clearLayers();
  if (!$("show-observations").checked || state.selected) return;
  for (const row of overview.singles) for (const area of observationAreas(row.raw)) {
    L.polygon(visibleRing(area), { color: "#7cc7ff", weight: 1, fill: false, className: "fix-region" })
      .on("click", () => selectObservation(row.id)).addTo(fixes);
  }
}

const groupCenter = group => group.rows.length > 1 ? group.bounds.getCenter() : group.anchor;

// Bins on opposite sides of a boundary can still put two count badges on top of one another.
// Merge their actual footprints and repeat if the new center meets another badge.
function separateGroups(groups) {
  const spacing = matchMedia("(pointer: coarse)").matches ? 44 : 32;
  let merged;
  do {
    merged = false;
    const cells = new Map(), kept = [];
    for (const group of groups) {
      const p = map.project(groupCenter(group));
      const x = Math.floor(p.x / spacing), y = Math.floor(p.y / spacing);
      let neighbor;
      for (let dx = -1; dx <= 1 && !neighbor; dx++) for (let dy = -1; dy <= 1 && !neighbor; dy++) {
        neighbor = cells.get([x + dx, y + dy].join("/"))?.find(other =>
          Math.abs(other.p.x - p.x) < spacing && Math.abs(other.p.y - p.y) < spacing);
      }
      if (neighbor) {
        neighbor.group.rows.push(...group.rows);
        neighbor.group.bounds.extend(group.bounds);
        merged = true;
      } else {
        const key = x + "/" + y;
        if (!cells.has(key)) cells.set(key, []);
        cells.get(key).push({ p, group });
        kept.push(group);
      }
    }
    groups = kept;
  } while (merged);
  return groups;
}

function drawTrackGroups() {
  const focused = document.activeElement?.closest(".overview-marker")?.dataset.group;
  tracksLayer.clearLayers();
  overview.wide = [];
  overview.drawnKeys = new Set();
  const groups = new Map();
  const view = map.getSize();
  for (const { row, area } of overview.tracks) {
    const bounds = L.latLngBounds(visibleRing(area));
    const pixels = map.project(bounds.getSouthEast()).subtract(map.project(bounds.getNorthWest()));
    if (pixels.x > Math.min(150, view.x * 0.3) || pixels.y > Math.min(105, view.y * 0.24)) {
      overview.wide.push(row);
      continue;
    }
    const anchor = visiblePoint(area.anchor);
    const point = map.project(anchor);
    const key = Math.floor(point.x / 60) + "/" + Math.floor(point.y / 44);
    let group = groups.get(key);
    if (!group) { group = { rows: [], bounds: L.latLngBounds([]), anchor }; groups.set(key, group); }
    group.rows.push(row);
    group.bounds.extend(bounds);
  }
  const wide = $("wide-group");
  wide.hidden = !overview.wide.length || !!state.selected;
  wide.textContent = "Spans regions · " + overview.wide.length;
  const ambiguous = $("ambiguous-map");
  ambiguous.hidden = !resultRows.ambiguous.length || !!state.selected;
  ambiguous.textContent = "Ambiguous · " + resultRows.ambiguous.length;
  if (state.selected) return;
  const visible = map.getBounds().pad(0.08);
  const hitSize = matchMedia("(pointer: coarse)").matches ? 44 : 24;
  for (const group of separateGroups([...groups.values()])) {
    if (!visible.intersects(group.bounds)) continue;
    for (const row of group.rows) overview.drawnKeys.add(row.id);
    const many = group.rows.length > 1, first = group.rows[0];
    const title = many ? group.rows.length + " located emitters in this area" : first.name;
    const html = many ? '<span class="area-count">' + group.rows.length + '</span>'
      : '<span class="area-symbol"></span><span class="tx-label">' + esc(first.name) + '</span>';
    const marker = L.marker(groupCenter(group), { title, autoPanOnFocus: false,
      icon: L.divIcon({ className: "overview-marker", html, iconSize: [hitSize, hitSize], iconAnchor: [hitSize / 2, hitSize / 2] }) })
      .on("click", () => many ? selectGroup("Located emitters", group.rows, group.bounds) : selectTrack(first.id)).addTo(tracksLayer);
    const el = marker.getElement();
    el.dataset.group = first.id;
    el.setAttribute("aria-label", title);
    el.addEventListener("keydown", e => {
      if (e.key === " " || e.key === "Enter") { e.preventDefault(); e.stopPropagation(); marker.fire("click"); }
    });
    let footprint;
    const preview = () => {
      if (footprint) return;
      const style = { color: "#71c9a9", weight: 1, fillOpacity: 0.05, interactive: false };
      footprint = (many ? L.rectangle(group.bounds, style) : L.polygon(visibleRing(trackArea(first.raw)), style)).addTo(tracksLayer);
    };
    const dismiss = () => { if (footprint) { tracksLayer.removeLayer(footprint); footprint = null; } };
    marker.on("mouseover", preview).on("mouseout", dismiss);
    el.addEventListener("focus", preview); el.addEventListener("blur", dismiss);
    if (focused === first.id) el.focus({ preventScroll: true });
  }
}

function placeLabels() {
  const bounds = $("map").getBoundingClientRect();
  const occupied = Array.from(document.querySelectorAll(".map-overlay > :not([hidden]), .leaflet-control, .area-count, .area-symbol, .rx-icon"), el => el.getBoundingClientRect());
  const collides = r => r.left < bounds.left || r.top < bounds.top || r.right > bounds.right || r.bottom > bounds.bottom ||
    occupied.some(b => r.left < b.right + 2 && r.right + 2 > b.left && r.top < b.bottom + 2 && r.bottom + 2 > b.top);
  for (const tag of document.querySelectorAll(".focus-tag")) {
    const w = tag.offsetWidth, h = tag.offsetHeight;
    const offsets = [[7, -h/2], [-w-7, -h/2], [-w/2, -h-7], [-w/2, 7], [7, -h-10], [-w-7, -h-10], [7, 10], [-w-7, 10]];
    for (const [x, y] of offsets) {
      tag.style.marginLeft = x + "px"; tag.style.marginTop = y + "px";
      if (!collides(tag.getBoundingClientRect())) break;
    }
    occupied.push(tag.getBoundingClientRect());
  }
  const labels = [...document.querySelectorAll(".tx-label"), ...document.querySelectorAll(".city")];
  for (const el of labels) {
    el.style.visibility = "";
    const r = el.getBoundingClientRect();
    if (collides(r)) el.style.visibility = "hidden";
    else occupied.push(r);
  }
}

function drawOverview() {
  drawLand();
  drawCities();
  drawPossibleAreas();
  drawTrackGroups();
  drawSingles();
  drawSelectionLayers();
  placeLabels();
}

function drawSelectionLayers() {
  selectedEvidence.clearLayers();
  receivers.clearLayers();
  const selection = state.selected;
  if (!selection) return;
  if (selection.bounds) L.rectangle(selection.bounds, { pane: "evidence", color: "#70d8b1", weight: 1, fill: false, interactive: false }).addTo(selectedEvidence);
  for (const a of selection.areas || []) {
    const hypothesis = a.type === "hypothesis";
    const color = hypothesis ? "#d2aaff" : a.type === "track" ? "#70dfb4" : "#89ceff";
    L.polygon(visibleRing(a), { pane: "evidence", color, weight: 2, fillOpacity: 0.11, interactive: false,
      dashArray: hypothesis ? "6 4" : null, className: "selected-" + a.type }).addTo(selectedEvidence);
    L.marker(visiblePoint(a.anchor), { interactive: false, keyboard: false, pane: "evidence",
      icon: L.divIcon({ className: "focus-tag " + a.type, html: esc(a.name), iconSize: null, iconAnchor: [-6, 10] }) }).addTo(selectedEvidence);
  }
  if (!$("show-receivers").checked) return;
  for (const r of selection.receivers || []) {
    const cls = r.used ? "used" : r.confirmed ? "heard" : "";
    L.marker(visiblePoint([r.position.lat, r.position.lon]), { title: r.label || r.id, keyboard: false,
      icon: L.divIcon({ className: "", html: '<div class="rx-icon ' + cls + '"></div>', iconSize: [12, 12] }) })
      .bindTooltip(esc(r.label || r.id) + (r.used ? " · used" : r.confirmed ? " · heard" : r.reason ? " · " + esc(r.reason) : ""))
      .addTo(receivers);
  }
}

function initializeMapView() {
  for (const [name, z] of [["geography", 310], ["possible", 390], ["evidence", 620]]) {
    const pane = map.createPane(name);
    pane.style.zIndex = z;
    pane.style.pointerEvents = "none";
  }
  coverageCanvas = L.DomUtil.create("canvas", "possible-coverage leaflet-zoom-hide", map.getPane("possible"));
  coverageCanvas.setAttribute("aria-hidden", "true");
  coverageMask = document.createElement("canvas"); coverageEdge = document.createElement("canvas");
  hitContext = document.createElement("canvas").getContext("2d");
  map.on("moveend resize", drawOverview);
  map.on("click", overviewClick);
  $("wide-group").onclick = () => selectGroup("Spans regions", overview.wide);
  $("ambiguous-map").onclick = () => resultRows.ambiguous.length === 1 ? selectObservation(resultRows.ambiguous[0].id) : selectGroup("Ambiguous observations", resultRows.ambiguous);
  $("map-key").ontoggle = placeLabels;
}
