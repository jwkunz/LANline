// NOAA Weather Radio transmitter directory (lazy-loaded asset).
// Regenerate with `node scripts/fetch-nwr.mjs`.

import { loadJson, nearest } from "./geo";

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

/** The seven NWR channel frequencies, in Hz. */
export const NWR_CHANNELS = [
  162_400_000, 162_425_000, 162_450_000, 162_475_000, 162_500_000, 162_525_000,
  162_550_000,
];

// Served from public/ next to index.html (see vite `base: "./"`).
export const loadNwrStations = () => loadJson<NwrStation[]>("./nwr-stations.json");

export const nearestNwr = (
  lat: number,
  lon: number,
  list: readonly NwrStation[],
  limit = 20,
) => nearest(lat, lon, list, limit);
