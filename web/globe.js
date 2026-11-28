// The flat map has no dependency on the orthographic renderer.
await import("./vendor/d3-array.min.js");
await import("./vendor/d3-geo.min.js");
const d3 = globalThis.d3;
const $ = id => document.getElementById(id);

function geometry(area) {
  const points = [];
  const ring = area.ring;
  for (let i = 0; i < ring.length; i++) {
    const [lat, lon] = ring[i], [nextLat, nextLon] = ring[(i + 1) % ring.length];
    points.push([lon, lat]);
    // A locator boundary follows a parallel. Subdivide long edges before spherical projection.
    if (area.grid && lat === nextLat) {
      const steps = Math.ceil(Math.abs(nextLon - lon) / 2);
      for (let j = 1; j < steps; j++) points.push([lon + (nextLon - lon) * j / steps, lat]);
    }
  }
  if (points.at(-1)[0] !== points[0][0] || points.at(-1)[1] !== points[0][1]) points.push(points[0]);
  const polygon = { type: "Polygon", coordinates: [points] };
  if (!d3.geoContains(polygon, [area.anchor[1], area.anchor[0]])) points.reverse();
  return polygon;
}

export function openGlobe(selection, world, onFlat) {
  const dialog = $("globe-dialog"), canvas = $("globe-canvas");
  if (dialog.open) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw Error("Canvas unavailable");
  const regions = (selection.areas || []).map(a => ({
    label: a.type === "hypothesis" ? "Region " + a.name : a.name,
    center: [a.anchor[1], a.anchor[0]], shape: geometry(a), type: a.type,
  }));
  const locations = [...regions, ...(selection.receivers || []).map(r => ({
    label: (r.label || r.id) + " · " + (r.used ? "used" : r.confirmed ? "heard" : "unconfirmed"),
    center: [r.position.lon, r.position.lat], type: r.used ? "used" : r.confirmed ? "heard" : "unconfirmed",
  }))];
  if (!locations.length) return;
  let [lon, lat] = locations[0].center;
  const projection = d3.geoOrthographic().clipAngle(90);
  const draw = d3.geoPath(projection, ctx), clipped = d3.geoPath(projection);
  const controls = $("globe-locations");
  controls.replaceChildren();
  $("globe-title").textContent = selection.title + " · Globe";
  $("globe-status").textContent = regions.some(r => r.type === "hypothesis")
    ? "Position undetermined · " + regions.length + " possible regions" : regions.length ? "Located · 95% model region" : "Heard without a region";
  $("globe-flat").textContent = regions.length > 1 ? "Show all regions on flat map" : "Show on flat map";
  for (const item of locations) {
    const b = document.createElement("button");
    b.type = "button";
    b.onclick = () => { [lon, lat] = item.center; render(); };
    controls.append(b);
    item.button = b;
  }
  function render() {
    const { width, height } = canvas.getBoundingClientRect();
    if (!width || !height) return;
    const ratio = Math.min(devicePixelRatio || 1, 2);
    const w = Math.round(width * ratio), h = Math.round(height * ratio);
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0); ctx.clearRect(0, 0, width, height);
    projection.rotate([-lon, -lat]).translate([width / 2, height / 2]).scale(Math.min(width, height) / 2 - 5);
    ctx.beginPath(); draw({ type: "Sphere" }); ctx.fillStyle = "#0d1b23"; ctx.fill(); ctx.strokeStyle = "#536b77"; ctx.lineWidth = 1; ctx.stroke();
    ctx.beginPath(); draw(world); ctx.fillStyle = "#2c424f"; ctx.fill(); ctx.strokeStyle = "#172a36"; ctx.lineWidth = 0.7; ctx.stroke();
    for (const item of locations) {
      const front = d3.geoDistance([lon, lat], item.center) <= Math.PI / 2;
      let visibility = front ? "visible" : "far side";
      if (item.shape) {
        if (!front && Number.isFinite(clipped.bounds(item.shape)[0][0])) visibility = "at horizon";
        ctx.beginPath(); draw(item.shape);
        ctx.strokeStyle = item.type === "hypothesis" ? "#d2aaff" : item.type === "track" ? "#70dfb4" : "#89ceff";
        ctx.fillStyle = ctx.strokeStyle; ctx.globalAlpha = 0.2; ctx.fill(); ctx.globalAlpha = 1;
        ctx.setLineDash(item.type === "hypothesis" ? [5, 3] : []); ctx.lineWidth = 2; ctx.stroke(); ctx.setLineDash([]);
        if (front) {
          const [x, y] = projection(item.center);
          ctx.fillStyle = "#eff8fc"; ctx.font = "600 12px system-ui"; ctx.fillText(item.label, x + 7, y - 6);
        }
      } else if (front) {
        const [x, y] = projection(item.center);
        ctx.beginPath(); ctx.arc(x, y, 4, 0, 2 * Math.PI);
        ctx.strokeStyle = item.type === "used" ? "#b9e3dd" : item.type === "heard" ? "#89ceff" : "#8aa0ad"; ctx.lineWidth = 1.5;
        if (item.type === "unconfirmed") ctx.setLineDash([2, 2]);
        if (item.type === "used") { ctx.fillStyle = "#5fd4a6"; ctx.fill(); }
        ctx.stroke(); ctx.setLineDash([]);
      }
      const text = item.label + " · " + visibility;
      if (item.button.textContent !== text) item.button.textContent = text;
    }
  }
  let last = null, frame = 0;
  function rotate(x, y) {
    lon = ((lon + x + 540) % 360) - 180;
    lat = Math.max(-90, Math.min(90, lat + y));
    if (!frame) frame = requestAnimationFrame(() => { frame = 0; render(); });
  }
  canvas.onpointerdown = e => { last = [e.clientX, e.clientY]; canvas.setPointerCapture(e.pointerId); };
  canvas.onpointermove = e => {
    if (!last) return;
    const scale = 130 / Math.max(1, canvas.clientHeight);
    rotate((last[0] - e.clientX) * scale, (e.clientY - last[1]) * scale);
    last = [e.clientX, e.clientY];
  };
  canvas.onlostpointercapture = () => { last = null; };
  canvas.onkeydown = e => {
    const step = { ArrowLeft: [-12, 0], ArrowRight: [12, 0], ArrowUp: [0, 12], ArrowDown: [0, -12] }[e.key];
    if (step) { e.preventDefault(); rotate(...step); }
  };
  $("globe-flat").onclick = () => { dialog.close(); onFlat(); };
  dialog.showModal();
  const observer = new ResizeObserver(render);
  observer.observe(canvas);
  dialog.addEventListener("close", () => { observer.disconnect(); cancelAnimationFrame(frame); }, { once: true });
  render();
}
