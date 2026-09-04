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

// Tunable range for the manual dial. The US channel plan is 88.1–107.9 MHz on
// odd tenths, but allow a wider span for edge/other-region stations; the
// 200 kHz grid stays anchored on 88.1 (…87.9, 87.7, … / …108.1, 108.3, …).
export const FM_MIN_HZ = 80_000_000;
export const FM_MAX_HZ = 110_000_000;
export const FM_STEP_HZ = 200_000;
const FM_ANCHOR_HZ = 88_100_000;

/** Snap a frequency (Hz) to the 200 kHz grid, clamped to the tunable range. */
export function snapFm(hz: number): number {
  const clamped = Math.min(FM_MAX_HZ, Math.max(FM_MIN_HZ, hz));
  const n = Math.round((clamped - FM_ANCHOR_HZ) / FM_STEP_HZ);
  return Math.min(FM_MAX_HZ, Math.max(FM_MIN_HZ, FM_ANCHOR_HZ + n * FM_STEP_HZ));
}

export function stepFm(hz: number, deltaChannels: number): number {
  return snapFm(hz + deltaChannels * FM_STEP_HZ);
}
