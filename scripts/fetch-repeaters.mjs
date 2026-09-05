#!/usr/bin/env node
// Regenerate web/public/repeaters.json — amateur VHF/UHF FM repeaters for a
// region (callsign, output frequency, offset, CTCSS uplink/downlink tones,
// transmitter lat/lon, town, open/closed).
//
//   node scripts/fetch-repeaters.mjs                       # default: FL
//   HAM_REPEATER_REGIONS="FL,GA,AL" node scripts/fetch-repeaters.mjs
//
// Source: hearham.com's open repeater API (one ~9 MB global JSON, no key, no
// rate limit) — https://hearham.com/api/repeaters/v1. The output is committed
// so builds need no network; RepeaterBook's export API now requires an
// account, hence hearham.
//
// Regions are matched against the free-form "city" string ("Gainesville, FL",
// "Miami, FL USA", …) by trailing state code, so pass 2-letter codes.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const OUT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../web/public/repeaters.json",
);
const SOURCE = "https://hearham.com/api/repeaters/v1";

const REGIONS = (process.env.HAM_REPEATER_REGIONS || "FL")
  .split(",")
  .map((s) => s.trim().toUpperCase())
  .filter(Boolean);

// Amateur VHF/UHF FM bands the `ham` mode covers, by output frequency (Hz).
const BANDS = [
  { id: "6m", lo: 50e6, hi: 54e6 },
  { id: "2m", lo: 144e6, hi: 148e6 },
  { id: "1.25m", lo: 222e6, hi: 225e6 },
  { id: "70cm", lo: 420e6, hi: 450e6 },
  { id: "33cm", lo: 902e6, hi: 928e6 },
  { id: "23cm", lo: 1240e6, hi: 1300e6 },
];
const bandOf = (hz) => BANDS.find((b) => hz >= b.lo && hz < b.hi) || null;

// "100.0" / "0.00" / "" / "D023" -> CTCSS Hz or 0 (none / DCS / carrier).
const tone = (v) => {
  const n = parseFloat(String(v ?? "").trim());
  return Number.isFinite(n) && n >= 60 && n <= 260 ? Math.round(n * 10) / 10 : 0;
};
const stateOf = (city) => {
  const m = String(city ?? "")
    .toUpperCase()
    .match(/,\s*([A-Z]{2})\b/);
  return m ? m[1] : "";
};

console.log(`fetch ${SOURCE}`);
const res = await fetch(SOURCE, { headers: { "User-Agent": "LANline/1.5 (+github.com/jwkunz/LANline)" } });
if (!res.ok) throw new Error(`fetch: HTTP ${res.status}`);
const rows = await res.json();
console.log(`  ${rows.length} repeaters worldwide; filtering to ${REGIONS.join(", ")}`);

const out = [];
for (const row of rows) {
  if (row.operational !== 1) continue;
  if (!/(^|[,\s])FM([,\s]|$)/i.test(String(row.mode || ""))) continue;

  const st = stateOf(row.city);
  if (!REGIONS.includes(st)) continue;

  const outputHz = Math.round(Number(row.frequency) || 0);
  const band = bandOf(outputHz);
  if (!band) continue;

  const call = String(row.callsign || "").trim().toUpperCase();
  if (!call) continue;

  const lat = Number(row.latitude);
  const lon = Number(row.longitude);
  const restriction = String(row.restriction || "").trim().toLowerCase();

  out.push({
    id: String(row.id),
    call,
    output_hz: outputHz,
    offset_hz: Math.round(Number(row.offset) || 0),
    tone_hz: tone(row.encode), // uplink (to key it — used by TX later)
    tsq_hz: tone(row.decode), // downlink (what it sends — optional RX squelch)
    lat: Number.isFinite(lat) ? Number(lat.toFixed(5)) : 0,
    lon: Number.isFinite(lon) ? Number(lon.toFixed(5)) : 0,
    place: String(row.city || "").replace(/\s+USA$/i, "").trim(),
    band: band.id,
    open: !restriction || restriction === "open",
  });
}

out.sort((a, b) => a.band.localeCompare(b.band) || a.output_hz - b.output_hz || a.call.localeCompare(b.call));
// Dedupe exact call+frequency collisions (multiple DB entries for one machine).
const seen = new Set();
const list = out.filter((r) => {
  const k = `${r.call}@${r.output_hz}`;
  return seen.has(k) ? false : seen.add(k);
});

writeFileSync(OUT, JSON.stringify(list) + "\n");
console.log(`wrote ${list.length} repeaters to ${OUT} (regions: ${REGIONS.join(", ")})`);
