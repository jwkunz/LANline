#!/usr/bin/env node
// Regenerate web/public/am-stations.json from the FCC AM Query — every
// licensed AM broadcast station in the US, with call sign, frequency, class,
// community of license, transmitter lat/lon and day/night power.
//
//   node scripts/fetch-am.mjs
//
// Output is committed so builds need no network.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const OUT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../web/public/am-stations.json",
);

const STATES = (
  "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS " +
  "MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI " +
  "WY PR VI GU AS MP"
).split(" ");

const q = (state) =>
  "https://transition.fcc.gov/fcc-bin/amq?" +
  new URLSearchParams({
    state,
    call: "",
    freq: "530", // kHz — the AM query wants integer kHz, not MHz like fmq
    fre2: "1710",
    serv: "AM",
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

    const freqKhz = parseFloat(f[2]);
    const lat = dms(f[20], f[21], f[22], f[19]);
    const lon = dms(f[24], f[25], f[26], f[23]);
    if (!Number.isFinite(freqKhz) || !Number.isFinite(lat) || !Number.isFinite(lon)) continue;
    if (lat === 0 && lon === 0) continue;

    const facId = f[18] || `${call}-${f[2]}`;
    const period = f[5]; // DAY / NIG / UNL / CH ("critical hours")
    const powerKw = parseFloat(f[14]);

    let entry = byFac.get(facId);
    if (!entry) {
      entry = {
        call,
        freq_hz: Math.round(freqKhz * 1000),
        class: f[7] || "",
        city: titleCase(f[10]),
        state: f[11],
        lat: Number(lat.toFixed(5)),
        lon: Number(lon.toFixed(5)),
        power_kw: null,
        power_night_kw: null,
      };
      byFac.set(facId, entry);
      kept++;
    }
    if (!Number.isFinite(powerKw)) continue;
    if (period === "NIG") {
      entry.power_night_kw = Number(powerKw.toFixed(3));
    } else if (period === "DAY" || period === "UNL" || entry.power_kw == null) {
      entry.power_kw = Number(powerKw.toFixed(3));
    }
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
console.log(`\nwrote ${stations.length} AM stations to ${OUT}`);
