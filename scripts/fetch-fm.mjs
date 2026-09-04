#!/usr/bin/env node
// Regenerate client/web/public/fm-stations.json from the FCC FM Query — every
// licensed full-service FM broadcast station in the US, with call sign,
// frequency, class, community of license and transmitter lat/lon.
//
//   node scripts/fetch-fm.mjs
//
// Output is committed so builds need no network.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const OUT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../client/web/public/fm-stations.json",
);

const STATES = (
  "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS " +
  "MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI " +
  "WY PR VI GU AS MP"
).split(" ");

const q = (state) =>
  "https://transition.fcc.gov/fcc-bin/fmq?" +
  new URLSearchParams({
    state,
    call: "",
    freq: "0.0",
    fre2: "108.0",
    serv: "FM",
    status: "",
    list: "4",
    NS: "N",
    EW: "W",
    size: "9",
  });

const dms = (d, m, s, hemi) => {
  const v = Math.abs(+d) + +m / 60 + +s / 3600;
  return hemi === "S" || hemi === "W" ? -v : v;
};

const byFac = new Map();
for (const st of STATES) {
  let text;
  try {
    const res = await fetch(q(st), { headers: { "user-agent": "lanline-fetch" } });
    if (!res.ok) {
      console.warn(`${st}: HTTP ${res.status}`);
      continue;
    }
    text = await res.text();
  } catch (e) {
    console.warn(`${st}: ${e.message}`);
    continue;
  }

  let kept = 0;
  for (const line of text.split("\n")) {
    const f = line.split("|").map((x) => x.trim());
    if (f.length < 27) continue;
    const call = f[1];
    const status = f[9];
    if (!call || call === "NEW" || call === "-") continue;
    if (status !== "LIC") continue; // licensed & operating only

    const freqMhz = parseFloat(f[2]);
    const lat = dms(f[20], f[21], f[22], f[19]);
    const lon = dms(f[24], f[25], f[26], f[23]);
    if (!Number.isFinite(freqMhz) || !Number.isFinite(lat) || !Number.isFinite(lon)) continue;
    if (lat === 0 && lon === 0) continue;

    const facId = f[18] || `${call}-${f[2]}`;
    if (byFac.has(facId)) continue;

    const erp = parseFloat(f[14]);
    byFac.set(facId, {
      call,
      freq_hz: Math.round(freqMhz * 1e6),
      class: f[7] || "",
      city: titleCase(f[10]),
      state: f[11],
      lat: Number(lat.toFixed(5)),
      lon: Number(lon.toFixed(5)),
      erp_kw: Number.isFinite(erp) && erp > 0 ? Number(erp.toFixed(2)) : null,
    });
    kept++;
  }
  console.log(`${st}: ${kept}`);
  await new Promise((r) => setTimeout(r, 250));
}

function titleCase(s) {
  return s
    .toLowerCase()
    .replace(/\b([a-z])/g, (m) => m.toUpperCase())
    .replace(/\b(Of|The|And)\b/g, (m) => m.toLowerCase());
}

const stations = [...byFac.values()].sort(
  (a, b) => a.freq_hz - b.freq_hz || a.call.localeCompare(b.call),
);
writeFileSync(OUT, JSON.stringify(stations) + "\n");
console.log(`\nwrote ${stations.length} FM stations to ${OUT}`);
