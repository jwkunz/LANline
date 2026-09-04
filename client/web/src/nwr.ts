// NOAA Weather Radio transmitter directory, bundled so it works offline.
// Regenerate with `node scripts/fetch-nwr.mjs`.

import stationsData from "./nwr-stations.json";

export interface NwrStation {
  callsign: string;
  freq_hz: number;
  lat: number;
  lon: number;
  site: string;
  state: string;
  power_w: number | null;
  status: "normal" | "degraded" | "out_of_service";
}

export interface NearbyStation extends NwrStation {
  distance_mi: number;
}

export const NWR_STATIONS = stationsData as NwrStation[];

/** The seven NWR channel frequencies, in Hz. */
export const NWR_CHANNELS = [
  162_400_000, 162_425_000, 162_450_000, 162_475_000, 162_500_000, 162_525_000,
  162_550_000,
];

function haversineMi(
  aLat: number,
  aLon: number,
  bLat: number,
  bLon: number,
): number {
  const R = 3958.7613; // mean earth radius, miles
  const toRad = (d: number) => (d * Math.PI) / 180;
  const dLat = toRad(bLat - aLat);
  const dLon = toRad(bLon - aLon);
  const s =
    Math.sin(dLat / 2) ** 2 +
    Math.cos(toRad(aLat)) * Math.cos(toRad(bLat)) * Math.sin(dLon / 2) ** 2;
  return 2 * R * Math.asin(Math.sqrt(s));
}

/** Transmitters nearest a point, closest first. */
export function nearestStations(
  lat: number,
  lon: number,
  limit = 20,
): NearbyStation[] {
  return NWR_STATIONS.map((s) => ({
    ...s,
    distance_mi: haversineMi(lat, lon, s.lat, s.lon),
  }))
    .sort((a, b) => a.distance_mi - b.distance_mi)
    .slice(0, limit);
}

/** Parse "lat, lon" / "lat lon" (also accepts N/S/E/W suffixes loosely). */
export function parseLatLon(text: string): { lat: number; lon: number } | null {
  const m = text
    .trim()
    .replace(/[NnEe]/g, "")
    .replace(/[SsWw]/g, "-")
    .match(/(-?\d+(?:\.\d+)?)\s*[, ]\s*(-?\d+(?:\.\d+)?)/);
  if (!m) return null;
  const lat = parseFloat(m[1]!);
  const lon = parseFloat(m[2]!);
  if (Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
  return { lat, lon };
}
