#!/usr/bin/env node
// Regenerate web/public/repeaters.json — amateur VHF/UHF FM repeaters
// (callsign, output frequency, offset, CTCSS uplink/downlink tones,
// transmitter lat/lon, town, open/closed).
//
//   node scripts/fetch-repeaters.mjs                          # default: all US
//   HAM_REPEATER_REGIONS="FL,GA,AL" node scripts/fetch-repeaters.mjs
//   HAM_REPEATER_CENTER="34.0,-81.0" \
//     HAM_REPEATER_RADIUS_MI=150 node scripts/fetch-repeaters.mjs
//
// The committed default is **nationwide** — same as fm-stations.json — so the
// web client filters/sorts it by your location client-side, wherever you are.
// The region / center overrides just make a smaller bundle if you want one.
//
// Source: hearham.com's open repeater API (one ~9 MB global JSON, no key, no
// rate limit) — https://hearham.com/api/repeaters/v1. The output is committed
// so builds need no network; RepeaterBook's export API now requires an
// account, hence hearham.
//
// hearham's "city" string comes in two shapes — "Town, FL USA" *and*
// "Town, Florida" — so region matching handles both a trailing 2-letter code
// and a spelled-out state name. If HAM_REPEATER_CENTER is set, that (plus
// HAM_REPEATER_RADIUS_MI) filters by transmitter distance instead, and the
// output is sorted nearest-first.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const OUT = resolve(dirname(fileURLToPath(import.meta.url)), "../web/public/repeaters.json");
const SOURCE = "https://hearham.com/api/repeaters/v1";

// Default: every US state + DC (nationwide, like fm-stations.json).
const US_ALL =
  "AL AK AZ AR CA CO CT DE DC FL GA HI ID IL IN IA KS KY LA ME MD MA MI MN MS " +
  "MO MT NE NV NH NJ NM NY NC ND OH OK OR PA RI SC SD TN TX UT VT VA WA WV WI WY";
const REGIONS = (process.env.HAM_REPEATER_REGIONS || US_ALL)
  .split(/[\s,]+/)
  .map((s) => s.trim().toUpperCase())
  .filter(Boolean);

const CENTER = (() => {
  const [la, lo] = (process.env.HAM_REPEATER_CENTER || "").split(",").map((v) => parseFloat(v));
  return Number.isFinite(la) && Number.isFinite(lo) ? { lat: la, lon: lo } : null;
})();
const RADIUS_MI = parseFloat(process.env.HAM_REPEATER_RADIUS_MI || "150");

// USPS code -> lowercase full name, for the "Town, Florida" city format.
// prettier-ignore
const STATE_NAMES = {
  AL:"alabama", AK:"alaska", AZ:"arizona", AR:"arkansas", CA:"california", CO:"colorado",
  CT:"connecticut", DE:"delaware", DC:"district of columbia", FL:"florida", GA:"georgia",
  HI:"hawaii", ID:"idaho", IL:"illinois", IN:"indiana", IA:"iowa", KS:"kansas", KY:"kentucky",
  LA:"louisiana", ME:"maine", MD:"maryland", MA:"massachusetts", MI:"michigan", MN:"minnesota",
  MS:"mississippi", MO:"missouri", MT:"montana", NE:"nebraska", NV:"nevada", NH:"new hampshire",
  NJ:"new jersey", NM:"new mexico", NY:"new york", NC:"north carolina", ND:"north dakota",
  OH:"ohio", OK:"oklahoma", OR:"oregon", PA:"pennsylvania", RI:"rhode island", SC:"south carolina",
  SD:"south dakota", TN:"tennessee", TX:"texas", UT:"utah", VT:"vermont", VA:"virginia",
  WA:"washington", WV:"west virginia", WI:"wisconsin", WY:"wyoming", PR:"puerto rico",
};
const VALID_CODES = new Set(Object.keys(STATE_NAMES));

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

/** USPS code for a hearham "city" string, or "" if none is recognizable. */
function stateOf(city) {
  const s = String(city ?? "")
    .toUpperCase()
    .replace(/,?\s*(USA|UNITED STATES)\s*$/i, "")
    .trim();
  const code = s.match(/(?:^|[,\s/])([A-Z]{2})\s*$/);
  if (code && VALID_CODES.has(code[1])) return code[1];
  const lc = s.toLowerCase();
  for (const [c, name] of Object.entries(STATE_NAMES)) {
    if (lc.endsWith(name) || lc.includes(`, ${name}`)) return c;
  }
  return "";
}

/** "Town, FL" for display — strip the country and normalize the state. */
function placeOf(city, code) {
  let town = String(city ?? "")
    .replace(/,?\s*(USA|United States)\s*$/i, "")
    .trim();
  if (code) {
    const name = STATE_NAMES[code];
    town = town
      .replace(new RegExp(`,?\\s*${name}\\s*$`, "i"), "")
      .replace(new RegExp(`,?\\s*${code}\\s*$`, "i"), "")
      .trim()
      .replace(/[,/]\s*$/, "");
    return town ? `${town}, ${code}` : code;
  }
  return town;
}

const R_MI = 3958.7613;
const rad = (d) => (d * Math.PI) / 180;
function haversineMi(aLat, aLon, bLat, bLon) {
  const dLat = rad(bLat - aLat);
  const dLon = rad(bLon - aLon);
  const x =
    Math.sin(dLat / 2) ** 2 +
    Math.cos(rad(aLat)) * Math.cos(rad(bLat)) * Math.sin(dLon / 2) ** 2;
  return 2 * R_MI * Math.asin(Math.sqrt(x));
}

console.log(`fetch ${SOURCE}`);
const res = await fetch(SOURCE, {
  headers: { "User-Agent": "LANline/1.9 (+github.com/jwkunz/LANline)" },
});
if (!res.ok) throw new Error(`fetch: HTTP ${res.status}`);
const rows = await res.json();
console.log(
  `  ${rows.length} repeaters worldwide; ` +
    (CENTER
      ? `within ${RADIUS_MI} mi of ${CENTER.lat},${CENTER.lon}`
      : `regions ${REGIONS.join(", ")}`),
);

/** Keep the entry with more filled-in fields when a machine is listed twice. */
const richness = (r) =>
  (r.tone_hz ? 1 : 0) + (r.tsq_hz ? 1 : 0) + (r.offset_hz ? 1 : 0) + (r.lat ? 1 : 0);

const byKey = new Map();
let scanned = 0;
for (const row of rows) {
  if (row.operational === 0) continue; // keep 1 and missing/unknown
  if (!/(^|[,\s/])FM([,\s/]|$)/i.test(String(row.mode || ""))) continue;

  const outputHz = Math.round(Number(row.frequency) || 0);
  const band = bandOf(outputHz);
  if (!band) continue;

  const call = String(row.callsign || "").trim().toUpperCase();
  if (!call) continue;

  const lat = Number(row.latitude);
  const lon = Number(row.longitude);
  const hasLL = Number.isFinite(lat) && Number.isFinite(lon) && (lat !== 0 || lon !== 0);
  const code = stateOf(row.city);

  let distance_mi = null;
  if (CENTER) {
    if (!hasLL) continue;
    distance_mi = haversineMi(CENTER.lat, CENTER.lon, lat, lon);
    if (distance_mi > RADIUS_MI) continue;
  } else if (!REGIONS.includes(code)) {
    continue;
  }
  scanned++;

  const restriction = String(row.restriction || "").trim().toLowerCase();
  const entry = {
    id: String(row.id),
    call,
    output_hz: outputHz,
    offset_hz: Math.round(Number(row.offset) || 0),
    tone_hz: tone(row.encode), // uplink (to key it)
    tsq_hz: tone(row.decode), // downlink (what it sends — optional RX squelch)
    lat: hasLL ? Number(lat.toFixed(5)) : 0,
    lon: hasLL ? Number(lon.toFixed(5)) : 0,
    place: placeOf(row.city, code),
    band: band.id,
    open: !restriction || restriction === "open",
  };
  if (CENTER) entry._d = distance_mi;

  const key = `${call}@${outputHz}`;
  const prev = byKey.get(key);
  if (!prev || richness(entry) > richness(prev)) byKey.set(key, entry);
}

let list = [...byKey.values()];
if (CENTER) {
  list.sort((a, b) => a._d - b._d);
  list.forEach((r) => delete r._d);
} else {
  list.sort(
    (a, b) =>
      a.band.localeCompare(b.band) || a.output_hz - b.output_hz || a.call.localeCompare(b.call),
  );
}

writeFileSync(OUT, JSON.stringify(list) + "\n");

const perBand = {};
for (const r of list) perBand[r.band] = (perBand[r.band] || 0) + 1;
const withTone = list.filter((r) => r.tone_hz).length;
const withOffset = list.filter((r) => r.offset_hz).length;
console.log(
  `wrote ${list.length} repeaters to ${OUT}\n` +
    `  bands: ${Object.entries(perBand)
      .map(([b, n]) => `${b} ${n}`)
      .join(", ")}\n` +
    `  ${withTone} with an uplink tone, ${withOffset} with an offset`,
);
