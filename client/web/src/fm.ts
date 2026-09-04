// US FM broadcast station directory (lazy-loaded asset).
// Regenerate with `node scripts/fetch-fm.mjs`.

import { loadJson, nearest } from "./geo";

export interface FmStation {
  call: string;
  freq_hz: number;
  class: string;
  city: string;
  state: string;
  lat: number;
  lon: number;
  erp_kw: number | null;
}

export const loadFmStations = () => loadJson<FmStation[]>("./fm-stations.json");

export const nearestFm = (
  lat: number,
  lon: number,
  list: readonly FmStation[],
  limit = 24,
) => nearest(lat, lon, list, limit);

// US FM band: 88.1–107.9 MHz on odd tenths (200 kHz spacing).
export const FM_MIN_HZ = 88_100_000;
export const FM_MAX_HZ = 107_900_000;
export const FM_STEP_HZ = 200_000;

/** Snap a frequency (Hz) to the nearest valid FM channel. */
export function snapFm(hz: number): number {
  const clamped = Math.min(FM_MAX_HZ, Math.max(FM_MIN_HZ, hz));
  const n = Math.round((clamped - FM_MIN_HZ) / FM_STEP_HZ);
  return FM_MIN_HZ + n * FM_STEP_HZ;
}

export function stepFm(hz: number, deltaChannels: number): number {
  return snapFm(hz + deltaChannels * FM_STEP_HZ);
}
