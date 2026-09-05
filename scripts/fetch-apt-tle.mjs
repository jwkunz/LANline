#!/usr/bin/env node
// Regenerate web/public/apt-tle.json from Celestrak — current orbital
// elements (TLEs) for the three active NOAA POES weather satellites that
// transmit APT on 137 MHz. The web client runs SGP4 locally (satellite.js)
// against this to predict upcoming passes; no live pass-prediction service
// is called at runtime.
//
//   node scripts/fetch-apt-tle.mjs
//
// TLEs drift out of date faster than the other station databases here
// (accurate for roughly 1-2 weeks) — rerun this periodically, ideally as
// part of a release, and don't expect months-old committed data to predict
// passes accurately.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const OUT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../web/public/apt-tle.json",
);

// NORAD catalog numbers for the three satellites still transmitting APT.
const SATELLITES = [
  { name: "NOAA-15", norad_id: 25338 },
  { name: "NOAA-18", norad_id: 28654 },
  { name: "NOAA-19", norad_id: 33591 },
];

const tles = [];
for (const sat of SATELLITES) {
  const url = `https://celestrak.org/NORAD/elements/gp.php?CATNR=${sat.norad_id}&FORMAT=TLE`;
  let text;
  try {
    const res = await fetch(url, { headers: { "user-agent": "lanline-fetch" } });
    if (!res.ok) {
      console.warn(`${sat.name}: HTTP ${res.status}`);
      continue;
    }
    text = await res.text();
  } catch (e) {
    console.warn(`${sat.name}: ${e.message}`);
    continue;
  }
  const lines = text.trim().split("\n").map((l) => l.trim());
  const line1 = lines.find((l) => l.startsWith("1 "));
  const line2 = lines.find((l) => l.startsWith("2 "));
  if (!line1 || !line2) {
    console.warn(`${sat.name}: couldn't find TLE lines in response: ${text.slice(0, 80)}`);
    continue;
  }
  tles.push({ name: sat.name, norad_id: sat.norad_id, line1, line2 });
  console.log(`${sat.name}: ok`);
  await new Promise((r) => setTimeout(r, 250));
}

writeFileSync(OUT, JSON.stringify(tles) + "\n");
console.log(`\nwrote ${tles.length}/${SATELLITES.length} TLEs to ${OUT}`);
