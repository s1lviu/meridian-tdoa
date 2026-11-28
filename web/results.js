"use strict";

const resultPage = 40;
const resultLimits = new Map();
let resultRows = { track: [], ambiguous: [], heard: [], single: [] };

function resultButton(row) {
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.id = row.id;
  button.dataset.kind = row.kind;
  button.innerHTML = '<span><span class="call"></span> <span class="meta"></span></span><span class="meta"></span>';
  button.onclick = () => row.kind === "track" ? selectTrack(row.id) : selectObservation(row.id);
  return button;
}

// Retain buttons by record ID so updating a timestamp or receiving another result preserves focus.
function renderRows(list, rows) {
  const existing = new Map(Array.from(list.children, li => [li.firstChild.dataset.id, li]));
  const focused = list.contains(document.activeElement) ? document.activeElement : null;
  let next = list.firstChild;
  for (const row of rows) {
    let li = existing.get(row.id);
    if (!li) {
      li = document.createElement("li");
      li.append(resultButton(row));
    }
    existing.delete(row.id);
    const fields = li.querySelectorAll(".call, .meta");
    [row.name, row.detail, row.time].forEach((value, i) => {
      if (fields[i].textContent !== value) fields[i].textContent = value;
    });
    if (li !== next) list.insertBefore(li, next);
    next = li.nextSibling;
  }
  for (const li of existing.values()) li.remove();
  if (focused?.isConnected && document.activeElement !== focused) focused.focus({ preventScroll: true });
}

function pageRows(kind, keepList) {
  let rows = resultRows[kind];
  const limit = resultLimits.get(kind) || resultPage;
  const list = $(kind + "-list");
  if (list === keepList || list.contains(document.activeElement)) {
    const remaining = new Map(rows.map(r => [r.id, r]));
    const reading = [];
    for (const item of list.children) {
      const id = item.firstChild.dataset.id;
      if (remaining.has(id)) { reading.push(remaining.get(id)); remaining.delete(id); }
    }
    rows = reading.concat([...remaining.values()]);
  }
  renderRows(list, rows.slice(0, limit));
  $(kind === "track" ? "located-count" : kind + "-count").textContent = rows.length;
  const more = $(kind + "-more");
  more.hidden = rows.length <= limit;
  more.onclick = () => { resultLimits.set(kind, limit + resultPage); pageRows(kind); };
}

function drawLists(profile, keepList) {
  const query = $("search").value.trim().toLowerCase();
  const matches = (row, hz) => !query || [row.name, freq(hz), String(hz), row.detail].some(s => s.toLowerCase().includes(query));
  const rows = { track: [], ambiguous: [], heard: [], single: [] };
  for (const item of state.tracks) {
    const { track: t, ellipse_95: el } = item;
    const row = { kind: "track", id: t.key, name: trackName(t), raw: item,
      detail: (t.band || freq(t.frequency_hz)) + " · " + t.fixes + (t.fixes === 1 ? " fix" : " fixes"),
      time: "±" + km(el.major_km) + " · " + ago(t.last_utc) };
    if (matches(row, t.frequency_hz)) rows.track.push(row);
  }
  const seen = new Set();
  for (const e of [...(state.data?.emissions || []), ...(state.data?.unlocated || [])]) {
    if (seen.has(e.id) || (profile !== "all" && e.profile !== profile)) continue;
    seen.add(e.id);
    const kind = e.estimate ? "single" : e.hypotheses?.length ? "ambiguous" : "heard";
    const row = { kind, id: e.id, name: label(e), raw: e,
      detail: kind === "ambiguous" ? e.hypotheses.length + " possible regions" : e.confirmed_count + " rx",
      time: utc(e.start_utc).slice(11, 19) };
    if (matches(row, e.frequency_hz)) rows[kind].push(row);
  }
  resultRows = rows;
  for (const kind of Object.keys(rows)) pageRows(kind, keepList);
  $("track-empty").hidden = rows.track.length > 0;
  $("track-empty").textContent = query ? "No matching emitter." : "No transmitter located in this window. A unique fix needs at least four receivers.";
}
