// Amateur (ham) VHF/UHF NBFM band plans. Unlike FRS's fixed 22-channel table,
// the ham bands are continuously tunable — what's fixed is the *convention*:
// which slices are FM simplex, which are repeater sub-bands, the calling
// frequencies, and where FM isn't used at all (CW/SSB weak-signal segments).
//
// The segment data is the ARRL band plan (a **voluntary** usage guide — the
// FCC's actual sub-band rules for VHF/UHF are far coarser). Local frequency
// coordinators (SERA, the state repeater councils, …) refine it further,
// especially on 70 cm. Treat it as a hint, not gospel.
//
// Privilege: every band here is open to **all U.S. license classes**
// (Technician and above) for FM voice. The class distinctions that matter
// are on HF, which this mode doesn't cover.

export interface BandSegment {
  loHz: number;
  hiHz: number;
  /** What the band plan reserves this slice for. */
  use: string;
  /** true when FM voice is conventional here (drives the simplex list + a
   *  "no FM" caution elsewhere). */
  fm: boolean;
}

export interface HamSpot {
  hz: number;
  name: string;
  note?: string;
}

export interface HamBand {
  id: string; // "2m"
  name: string; // "2 meters"
  rangeLabel: string; // "144–148 MHz"
  loHz: number;
  hiHz: number;
  /** Sensible landing frequency when you first pick the band. */
  defaultHz: number;
  /** Repeater output ± input spacing on this band (informational until PTT). */
  repeaterOffsetHz: number;
  /** Named calling / common simplex frequencies. */
  spots: HamSpot[];
  /** Regular FM simplex grid(s): [loHz, hiHz, stepHz]. */
  simplexGrids: [number, number, number][];
  segments: BandSegment[];
}

const M = 1_000_000;
const K = 1_000;

export const HAM_BANDS: HamBand[] = [
  {
    id: "6m",
    name: "6 meters",
    rangeLabel: "50–54 MHz",
    loHz: 50 * M,
    hiHz: 54 * M,
    defaultHz: 52_525 * K,
    repeaterOffsetHz: -1 * M, // regional; often -500 kHz or -1 MHz
    spots: [
      { hz: 50_125 * K, name: "SSB calling", note: "USB — not FM" },
      { hz: 52_525 * K, name: "FM simplex calling" },
      { hz: 52_540 * K, name: "FM simplex" },
    ],
    simplexGrids: [[52_500 * K, 52_980 * K, 20 * K]],
    segments: [
      { loHz: 50.0 * M, hiHz: 50.1 * M, use: "CW only", fm: false },
      { loHz: 50.1 * M, hiHz: 50.3 * M, use: "SSB / weak-signal (50.125 calling)", fm: false },
      { loHz: 50.3 * M, hiHz: 50.6 * M, use: "all-mode, digital", fm: false },
      { loHz: 50.6 * M, hiHz: 51.0 * M, use: "experimental, misc", fm: false },
      { loHz: 51.0 * M, hiHz: 51.1 * M, use: "pacific DX window", fm: false },
      { loHz: 51.12 * M, hiHz: 51.48 * M, use: "repeater inputs / pairs", fm: true },
      { loHz: 51.5 * M, hiHz: 51.6 * M, use: "simplex", fm: true },
      { loHz: 52.0 * M, hiHz: 52.48 * M, use: "repeater pairs", fm: true },
      { loHz: 52.5 * M, hiHz: 52.98 * M, use: "FM simplex (52.525 calling)", fm: true },
      { loHz: 53.0 * M, hiHz: 53.48 * M, use: "repeater pairs", fm: true },
      { loHz: 53.5 * M, hiHz: 53.98 * M, use: "simplex, remote base", fm: true },
    ],
  },
  {
    id: "2m",
    name: "2 meters",
    rangeLabel: "144–148 MHz",
    loHz: 144 * M,
    hiHz: 148 * M,
    defaultHz: 146_520 * K,
    repeaterOffsetHz: 600 * K, // ±600 kHz standard
    spots: [
      { hz: 144_200 * K, name: "SSB calling", note: "USB — not FM" },
      { hz: 145_510 * K, name: "FM simplex" },
      { hz: 146_520 * K, name: "National FM simplex calling" },
      { hz: 146_550 * K, name: "FM simplex" },
      { hz: 146_580 * K, name: "FM simplex" },
      { hz: 147_420 * K, name: "FM simplex" },
      { hz: 147_555 * K, name: "FM simplex" },
    ],
    simplexGrids: [
      [145_510 * K, 145_790 * K, 20 * K],
      [146_400 * K, 146_580 * K, 15 * K],
      [147_420 * K, 147_570 * K, 15 * K],
    ],
    segments: [
      { loHz: 144.0 * M, hiHz: 144.05 * M, use: "EME / CW", fm: false },
      { loHz: 144.05 * M, hiHz: 144.1 * M, use: "general CW", fm: false },
      { loHz: 144.1 * M, hiHz: 144.2 * M, use: "EME / weak-signal CW", fm: false },
      { loHz: 144.2 * M, hiHz: 144.275 * M, use: "SSB weak-signal (144.200 calling)", fm: false },
      { loHz: 144.275 * M, hiHz: 144.3 * M, use: "beacons", fm: false },
      { loHz: 144.3 * M, hiHz: 144.5 * M, use: "OSCAR / simplex", fm: true },
      { loHz: 144.5 * M, hiHz: 144.6 * M, use: "linear translator inputs, misc", fm: true },
      { loHz: 144.6 * M, hiHz: 144.9 * M, use: "FM repeater inputs", fm: true },
      { loHz: 144.9 * M, hiHz: 145.1 * M, use: "weak-signal, packet, experimental", fm: false },
      { loHz: 145.1 * M, hiHz: 145.2 * M, use: "linear translator outputs, packet", fm: true },
      { loHz: 145.2 * M, hiHz: 145.5 * M, use: "FM repeater outputs", fm: true },
      { loHz: 145.5 * M, hiHz: 145.8 * M, use: "misc / experimental, FM simplex", fm: true },
      { loHz: 145.8 * M, hiHz: 146.0 * M, use: "OSCAR satellite", fm: false },
      { loHz: 146.01 * M, hiHz: 146.4 * M, use: "FM repeater inputs", fm: true },
      { loHz: 146.4 * M, hiHz: 146.6 * M, use: "FM simplex (146.52 calling)", fm: true },
      { loHz: 146.61 * M, hiHz: 147.0 * M, use: "FM repeater outputs", fm: true },
      { loHz: 147.0 * M, hiHz: 147.4 * M, use: "FM repeater outputs", fm: true },
      { loHz: 147.42 * M, hiHz: 147.57 * M, use: "FM simplex", fm: true },
      { loHz: 147.6 * M, hiHz: 147.99 * M, use: "FM repeater inputs", fm: true },
    ],
  },
  {
    id: "1.25m",
    name: "1.25 meters",
    rangeLabel: "222–225 MHz",
    loHz: 222 * M,
    hiHz: 225 * M,
    defaultHz: 223_500 * K,
    repeaterOffsetHz: -1_600 * K,
    spots: [
      { hz: 223_500 * K, name: "FM simplex calling" },
      { hz: 222_100 * K, name: "SSB / CW calling", note: "not FM" },
    ],
    simplexGrids: [[223_400 * K, 223_520 * K, 20 * K]],
    segments: [
      { loHz: 222.0 * M, hiHz: 222.15 * M, use: "weak-signal (222.100 calling)", fm: false },
      { loHz: 222.15 * M, hiHz: 222.25 * M, use: "weak-signal, propagation beacons", fm: false },
      { loHz: 222.25 * M, hiHz: 223.38 * M, use: "FM repeater inputs (−1.6 MHz)", fm: true },
      { loHz: 223.4 * M, hiHz: 223.52 * M, use: "FM simplex (223.500 calling)", fm: true },
      { loHz: 223.52 * M, hiHz: 223.64 * M, use: "digital, packet", fm: true },
      { loHz: 223.64 * M, hiHz: 223.7 * M, use: "links, control", fm: true },
      { loHz: 223.71 * M, hiHz: 223.85 * M, use: "FM repeater outputs", fm: true },
      { loHz: 223.85 * M, hiHz: 225.0 * M, use: "FM repeater outputs", fm: true },
    ],
  },
  {
    id: "70cm",
    name: "70 centimeters",
    rangeLabel: "420–450 MHz",
    loHz: 420 * M,
    hiHz: 450 * M,
    defaultHz: 446_000 * K,
    repeaterOffsetHz: 5 * M, // ±5 MHz standard
    spots: [
      { hz: 432_100 * K, name: "SSB / CW calling", note: "not FM" },
      { hz: 446_000 * K, name: "FM simplex calling" },
      { hz: 446_025 * K, name: "FM simplex" },
      { hz: 446_050 * K, name: "FM simplex" },
    ],
    simplexGrids: [[445_950 * K, 446_175 * K, 25 * K]],
    segments: [
      { loHz: 420 * M, hiHz: 426 * M, use: "ATV", fm: false },
      { loHz: 426 * M, hiHz: 432 * M, use: "ATV, digital", fm: false },
      { loHz: 432.0 * M, hiHz: 432.07 * M, use: "EME", fm: false },
      { loHz: 432.07 * M, hiHz: 432.1 * M, use: "weak-signal CW", fm: false },
      { loHz: 432.1 * M, hiHz: 432.3 * M, use: "SSB / CW (432.100 calling)", fm: false },
      { loHz: 432.3 * M, hiHz: 433.0 * M, use: "beacons, weak-signal", fm: false },
      { loHz: 433.0 * M, hiHz: 435.0 * M, use: "aux/links, FM simplex, repeaters", fm: true },
      { loHz: 435.0 * M, hiHz: 438.0 * M, use: "satellite only — no terrestrial", fm: false },
      { loHz: 438.0 * M, hiHz: 442.0 * M, use: "ATV repeaters, links, FM simplex", fm: true },
      { loHz: 442.0 * M, hiHz: 445.0 * M, use: "FM repeaters (outputs; inputs +5 MHz)", fm: true },
      { loHz: 445.0 * M, hiHz: 447.0 * M, use: "shared: simplex, control, misc", fm: true },
      { loHz: 447.0 * M, hiHz: 450.0 * M, use: "FM repeaters (−5 MHz offset)", fm: true },
    ],
  },
  {
    id: "33cm",
    name: "33 centimeters",
    rangeLabel: "902–928 MHz",
    loHz: 902 * M,
    hiHz: 928 * M,
    defaultHz: 927_500 * K,
    repeaterOffsetHz: -25 * M,
    spots: [
      { hz: 903_100 * K, name: "SSB / CW calling", note: "not FM" },
      { hz: 927_500 * K, name: "FM simplex calling" },
    ],
    simplexGrids: [[927_400 * K, 927_600 * K, 25 * K]],
    segments: [
      { loHz: 902.0 * M, hiHz: 903.0 * M, use: "weak-signal, EME", fm: false },
      { loHz: 903.0 * M, hiHz: 906.0 * M, use: "SSB/CW (903.100 calling), digital", fm: false },
      { loHz: 906.0 * M, hiHz: 909.0 * M, use: "FM simplex, digital", fm: true },
      { loHz: 909.0 * M, hiHz: 915.0 * M, use: "ATV, wideband", fm: false },
      { loHz: 918.0 * M, hiHz: 922.0 * M, use: "digital, links", fm: true },
      { loHz: 927.0 * M, hiHz: 928.0 * M, use: "FM simplex / repeater outputs (−25 MHz)", fm: true },
    ],
  },
  {
    id: "23cm",
    name: "23 centimeters",
    rangeLabel: "1240–1300 MHz",
    loHz: 1240 * M,
    hiHz: 1300 * M,
    defaultHz: 1_294_500 * K,
    repeaterOffsetHz: -12 * M,
    spots: [
      { hz: 1_296_100 * K, name: "SSB / CW calling", note: "not FM" },
      { hz: 1_294_500 * K, name: "FM simplex calling" },
    ],
    simplexGrids: [[1_294_000 * K, 1_295_000 * K, 25 * K]],
    segments: [
      { loHz: 1240 * M, hiHz: 1246 * M, use: "ATV #1", fm: false },
      { loHz: 1246 * M, hiHz: 1252 * M, use: "FM simplex, digital, links", fm: true },
      { loHz: 1252 * M, hiHz: 1258 * M, use: "ATV #2, digital", fm: false },
      { loHz: 1258 * M, hiHz: 1260 * M, use: "FM simplex, digital", fm: true },
      { loHz: 1270 * M, hiHz: 1276 * M, use: "FM repeater inputs (−12 MHz)", fm: true },
      { loHz: 1282 * M, hiHz: 1288 * M, use: "FM repeater outputs", fm: true },
      { loHz: 1290 * M, hiHz: 1294 * M, use: "FM simplex", fm: true },
      { loHz: 1294 * M, hiHz: 1295 * M, use: "FM simplex (1294.500 calling)", fm: true },
      { loHz: 1295 * M, hiHz: 1297 * M, use: "SSB/CW, weak-signal (1296.100 calling)", fm: false },
    ],
  },
];

export const HAM_DEFAULT_BAND = "2m";

export function hamBand(id: string): HamBand {
  return HAM_BANDS.find((b) => b.id === id) ?? HAM_BANDS[1]!;
}

/** All conventional FM simplex frequencies for a band: the named spots plus
 *  the regular grids, de-duped and sorted. */
export function hamSimplexChannels(band: HamBand): HamSpot[] {
  const out = new Map<number, HamSpot>();
  for (const s of band.spots) if (!s.note?.includes("not FM")) out.set(s.hz, s);
  for (const [lo, hi, step] of band.simplexGrids) {
    for (let hz = lo; hz <= hi + 1; hz += step) {
      if (!out.has(hz)) out.set(hz, { hz, name: `${(hz / 1e6).toFixed(4)} MHz` });
    }
  }
  return [...out.values()].sort((a, b) => a.hz - b.hz);
}

/** The band-plan segment a frequency falls in, if any. */
export function hamSegmentAt(band: HamBand, hz: number): BandSegment | undefined {
  return band.segments.find((s) => hz >= s.loHz && hz < s.hiHz);
}
