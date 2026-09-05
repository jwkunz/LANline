// NOAA APT (137 MHz weather satellite) client-side types + binary image
// decode. Mirrors the server's `apt::Status` JSON and `apt::Image::encode()`
// raster (see docs/rest-api.md).

import {
  degreesToRadians,
  ecfToLookAngles,
  eciToEcf,
  gstime,
  propagate,
  radiansToDegrees,
  twoline2satrec,
} from "satellite.js";
import { loadJson } from "./geo";

export interface AptStatus {
  width: number;
  height: number;
  lines: number;
  sync_quality: number;
}

export interface AptImage {
  width: number;
  height: number;
  /** Row-major grayscale, one byte per pixel (channel A then channel B per row). */
  pixels: Uint8Array;
}

/** Parse `GET /api/v1/apt/image`'s `[u32 width LE][u32 height LE][bytes]`. */
export function parseAptImage(buf: ArrayBuffer): AptImage {
  const view = new DataView(buf);
  const width = buf.byteLength >= 4 ? view.getUint32(0, true) : 0;
  const height = buf.byteLength >= 8 ? view.getUint32(4, true) : 0;
  const want = width * height;
  const have = Math.max(0, buf.byteLength - 8);
  const pixels = new Uint8Array(buf, 8, Math.min(want, have));
  return { width, height, pixels };
}

export interface AptSatellite {
  name: string;
  freq_hz: number;
}

/** NOAA POES 137 MHz APT downlinks. Whether any of these is actually
 *  receivable right now depends on that specific satellite being above the
 *  horizon for a pass — unlike AM/FM/ADS-B/AIS, which all have a broadcaster
 *  (or many aircraft/vessels) continuously in range, APT is a real-time feed
 *  from one spacecraft with no store-and-forward. */
export const APT_SATELLITES: AptSatellite[] = [
  { name: "NOAA-19", freq_hz: 137_100_000 },
  { name: "NOAA-18", freq_hz: 137_912_500 },
  { name: "NOAA-15", freq_hz: 137_620_000 },
];

export const APT_MIN_HZ = 137_000_000;
export const APT_MAX_HZ = 138_000_000;

// --- pass prediction (local SGP4, no live pass-prediction service) --------

export interface AptTle {
  name: string;
  norad_id: number;
  line1: string;
  line2: string;
}

/** Regenerate with `node scripts/fetch-apt-tle.mjs`. Unlike the station
 *  directories, this goes stale within 1-2 weeks — a TLE's accuracy decays
 *  with its age, so predictions from a long-uncommitted snapshot drift off. */
export const loadAptTle = () => loadJson<AptTle[]>("./apt-tle.json");

export interface AptPass {
  /** Epoch ms. */
  aos: number;
  los: number;
  max_elevation_deg: number;
}

/** Upcoming passes above `minElevationDeg`, computed locally from the TLE
 *  via SGP4 (satellite.js) — no network call, no external prediction
 *  service, works offline once the TLE is loaded. `withinHours` bounds how
 *  far ahead to search; a satellite with no qualifying pass in that window
 *  (a real possibility — ~14 orbits/day doesn't mean 14 usable overhead
 *  passes for any one location) simply returns fewer than `maxPasses`. */
export function nextPasses(
  tle: AptTle,
  lat: number,
  lon: number,
  withinHours = 48,
  minElevationDeg = 15,
  maxPasses = 3,
): AptPass[] {
  let satrec;
  try {
    satrec = twoline2satrec(tle.line1, tle.line2);
  } catch {
    return [];
  }
  const observerGd = { longitude: degreesToRadians(lon), latitude: degreesToRadians(lat), height: 0 };
  const stepMs = 30_000;
  const start = Date.now();
  const end = start + withinHours * 3_600_000;
  const passes: AptPass[] = [];
  let inPass = false;
  let aos = 0;
  let maxEl = -90;
  for (let t = start; t <= end && passes.length < maxPasses; t += stepMs) {
    const date = new Date(t);
    const pv = propagate(satrec, date);
    if (!pv?.position) continue; // decayed element set / propagation error
    const look = ecfToLookAngles(observerGd, eciToEcf(pv.position, gstime(date)));
    const elDeg = radiansToDegrees(look.elevation);
    const above = elDeg >= minElevationDeg;
    if (above && !inPass) {
      inPass = true;
      aos = t;
      maxEl = elDeg;
    } else if (above) {
      maxEl = Math.max(maxEl, elDeg);
    } else if (inPass) {
      inPass = false;
      passes.push({ aos, los: t, max_elevation_deg: Math.round(maxEl) });
    }
  }
  return passes;
}
