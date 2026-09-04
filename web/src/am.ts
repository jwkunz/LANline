// US AM (mediumwave) broadcast station directory (lazy-loaded asset).
// Regenerate with `node scripts/fetch-am.mjs`.

import { loadJson, nearest } from "./geo";

export interface AmStation {
  call: string;
  freq_hz: number;
  class: string;
  city: string;
  state: string;
  lat: number;
  lon: number;
  power_kw: number | null;
  power_night_kw: number | null;
}

export const loadAmStations = () => loadJson<AmStation[]>("./am-stations.json");

export const nearestAm = (
  lat: number,
  lon: number,
  list: readonly AmStation[],
  limit = 24,
) => nearest(lat, lon, list, limit);

// US mediumwave channel plan: 530–1700 kHz on a 10 kHz grid; allow a little
// margin either side for the manual dial (some other-region/expanded-band
// stations sit just outside).
export const AM_MIN_HZ = 520_000;
export const AM_MAX_HZ = 1_710_000;
export const AM_STEP_HZ = 10_000;
const AM_ANCHOR_HZ = 530_000;

/** Snap a frequency (Hz) to the 10 kHz grid, clamped to the tunable range. */
export function snapAm(hz: number): number {
  const clamped = Math.min(AM_MAX_HZ, Math.max(AM_MIN_HZ, hz));
  const n = Math.round((clamped - AM_ANCHOR_HZ) / AM_STEP_HZ);
  return Math.min(AM_MAX_HZ, Math.max(AM_MIN_HZ, AM_ANCHOR_HZ + n * AM_STEP_HZ));
}

export function stepAm(hz: number, deltaChannels: number): number {
  return snapAm(hz + deltaChannels * AM_STEP_HZ);
}
