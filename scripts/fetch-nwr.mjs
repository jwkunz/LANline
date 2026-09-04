#!/usr/bin/env node
// Regenerate client/web/src/nwr-stations.json from the NWS county-coverage
// dataset (CCL.js), which carries call sign, frequency, transmitter lat/lon,
// power and status for every NOAA Weather Radio transmitter.
//
//   node scripts/fetch-nwr.mjs
//
// The output is committed so builds need no network.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const SOURCE = "https://www.weather.gov/source/nwr/JS/CCL.js";
const OUT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../client/web/src/nwr-stations.json",
);

const ARRAYS = [
  "ST", "STATE", "COUNTY", "SAME", "SITENAME", "SITELOC", "SITESTATE",
  "FREQ", "CALLSIGN", "LAT", "LON", "PWR", "STATUS", "WFO", "REMARKS",
];

const res = await fetch(SOURCE);
if (!res.ok) throw new Error(`fetch ${SOURCE}: ${res.status}`);
const src = await res.text();

// CCL.js is nothing but `var X = []` declarations and `X[i] = "..."` lines.
// Evaluate it in a bare function scope and read the arrays back out.
const cols = new Function(`${src}\nreturn { ${ARRAYS.join(", ")} };`)();

const n = cols.CALLSIGN.length;
const byCall = new Map();
for (let i = 0; i < n; i++) {
  const callsign = (cols.CALLSIGN[i] || "").trim();
  const freqMhz = parseFloat(cols.FREQ[i]);
  const lat = parseFloat(cols.LAT[i]);
  const lon = parseFloat(cols.LON[i]);
  if (!callsign || !Number.isFinite(freqMhz) || !Number.isFinite(lat) || !Number.isFinite(lon)) {
    continue;
  }
  if (byCall.has(callsign)) continue; // one row per transmitter

  const statusRaw = (cols.STATUS[i] || "").trim().toUpperCase();
  const status =
    statusRaw === "OUT OF SERVICE" ? "out_of_service" :
    statusRaw === "DEGRADED" ? "degraded" : "normal";
  const powerW = parseInt(cols.PWR[i], 10);

  byCall.set(callsign, {
    callsign,
    freq_hz: Math.round(freqMhz * 1e6),
    lat: Number(lat.toFixed(5)),
    lon: Number(lon.toFixed(5)),
    site: (cols.SITELOC[i] || cols.SITENAME[i] || "").trim(),
    state: (cols.SITESTATE[i] || cols.ST[i] || "").trim(),
    power_w: Number.isFinite(powerW) && powerW > 0 ? powerW : null,
    status,
  });
}

const stations = [...byCall.values()].sort((a, b) => a.callsign.localeCompare(b.callsign));
writeFileSync(OUT, JSON.stringify(stations) + "\n");
console.log(`wrote ${stations.length} transmitters to ${OUT} (source: ${SOURCE})`);
