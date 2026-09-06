import { loadJson } from "./geo";

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

/** The 50 standard EIA/TIA-603 CTCSS ("PL") sub-audible tones, Hz. A repeater
 *  that requires a tone almost always uses one of these; the server's
 *  detector is a single-tone presence check, so the operator picks the one
 *  the repeater publishes. `0` = no tone (carrier squelch). */
// prettier-ignore
export const CTCSS_TONES: number[] = [
  67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5,
  94.8, 97.4, 100.0, 103.5, 107.2, 110.9, 114.8, 118.8, 123.0, 127.3,
  131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2, 165.5, 167.9,
  171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5,
  203.5, 206.5, 210.7, 218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

/** The standard DCS / DPL codes — the 3 octal digits written as a decimal
 *  number (23 → D023). Radios also offer each "inverted" (D023I); that's the
 *  `dcs_invert` flag, not a separate entry. */
// prettier-ignore
export const DCS_CODES: number[] = [
  23, 25, 26, 31, 32, 43, 47, 51, 54, 65, 71, 72, 73, 74,
  114, 115, 116, 122, 125, 131, 132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174,
  205, 212, 223, 225, 226, 243, 244, 245, 246, 251, 252, 255, 261, 263, 265, 266, 271, 274,
  306, 311, 315, 325, 331, 332, 343, 346, 351, 356, 364, 365, 371,
  411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
  503, 506, 516, 523, 526, 532, 546, 565,
  606, 612, 624, 627, 631, 632, 654, 662, 664,
  703, 712, 723, 731, 732, 734, 743, 754,
];

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

/** The band a raw frequency belongs to, or null if it's out of all of them. */
export function hamBandOf(hz: number): HamBand | null {
  return HAM_BANDS.find((b) => hz >= b.loHz && hz <= b.hiHz) ?? null;
}

// --- repeaters ------------------------------------------------------------

/** One repeater. `output_hz` is what you receive (the downlink); the input
 *  (uplink, for TX later) is `output_hz + offset_hz`. `tone_hz` is the CTCSS
 *  the machine needs to hear you (uplink); `tsq_hz` is the tone it transmits
 *  on its output, if any (0 = none) — the one worth using as RX tone squelch.
 *  `manual` rows are the operator's own, kept in localStorage. */
export interface Repeater {
  id: string;
  call: string;
  output_hz: number;
  offset_hz: number;
  tone_hz: number;
  tsq_hz: number;
  lat: number;
  lon: number;
  place: string;
  band: string;
  open: boolean;
  manual?: boolean;
}

/** Bundled regional list — regenerate with `node scripts/fetch-repeaters.mjs`. */
export const loadRepeaters = () => loadJson<Repeater[]>("./repeaters.json");

/** "−600 kHz" / "+5 MHz" / "simplex" — the split for a repeater's offset. */
export function offsetLabel(offsetHz: number): string {
  if (!offsetHz) return "simplex";
  const sign = offsetHz < 0 ? "−" : "+";
  const abs = Math.abs(offsetHz);
  return abs >= 1_000_000
    ? `${sign}${(abs / 1e6).toFixed(abs % 1e6 ? 2 : 0)} MHz`
    : `${sign}${Math.round(abs / 1e3)} kHz`;
}

/** Parse a spoken/written repeater shorthand into whatever fields it carries
 *  — for the "add repeater" quick box. Tolerant of order and punctuation:
 *
 *    146.94 - 100.0            output 146.94, conventional − offset, PL 100.0
 *    442.100 +5 PL 131.8 W4ABC output, +5 MHz, tone 131.8, call W4ABC
 *    147.120 +0.6 107.2        output, +600 kHz, tone 107.2
 *
 *  A signed magnitude ≥ 100 is read as kHz, otherwise MHz; a bare `+`/`-` is
 *  the band's conventional offset with that sign; a bare number in 60–260 is
 *  a CTCSS tone; a callsign-shaped token is the call. */
export function parseRepeaterShorthand(text: string): {
  output_hz?: number;
  offset_hz?: number;
  tone_hz?: number;
  tsq_hz?: number;
  call?: string;
} {
  const out: ReturnType<typeof parseRepeaterShorthand> = {};
  const toks = text.trim().split(/[\s,]+/).filter(Boolean);
  let sign: 1 | -1 | 0 = 0;
  let toneKindNext: "tone" | "tsq" | null = null;

  for (const raw of toks) {
    const t = raw.trim();
    if (/^[+]$/.test(t)) { sign = 1; continue; }
    if (/^[-]$/.test(t)) { sign = -1; continue; }
    if (/^(pl|t|tone|ctcss)$/i.test(t)) { toneKindNext = "tone"; continue; }
    if (/^(tsq|rx|dcs|dpl)$/i.test(t)) { toneKindNext = "tsq"; continue; }
    if (/^[A-Z]{1,2}\d[A-Z0-9]{1,4}$/i.test(t)) { out.call = t.toUpperCase(); continue; }

    const n = parseFloat(t);
    if (!Number.isFinite(n)) continue;
    const signed = /^[+-]/.test(t);

    if (toneKindNext) {
      if (n >= 60 && n <= 260) out[toneKindNext === "tone" ? "tone_hz" : "tsq_hz"] = Math.round(n * 10) / 10;
      toneKindNext = null;
      continue;
    }
    if (signed) {
      // offset: |n| >= 100 -> kHz, else MHz
      out.offset_hz = Math.round((Math.abs(n) >= 100 ? n * 1e3 : n * 1e6));
      continue;
    }
    if (out.output_hz == null) {
      out.output_hz = n > 1e6 ? Math.round(n) : Math.round(n * 1e6);
      continue;
    }
    if (n >= 60 && n <= 260 && out.tone_hz == null) {
      out.tone_hz = Math.round(n * 10) / 10;
    }
  }

  if (sign !== 0 && out.offset_hz == null && out.output_hz != null) {
    out.offset_hz = sign * Math.abs(conventionalOffsetHz(out.output_hz));
  }
  return out;
}

/** Conventional repeater offset for a 2 m / 70 cm / … output frequency —
 *  used to pre-fill the "add repeater" form. Sign follows the usual sub-band
 *  convention (e.g. 2 m outputs 145.2–145.5 are −600 kHz, 146.61–147.00 are
 *  −600 kHz, 147.00–147.40 are +600 kHz). */
export function conventionalOffsetHz(outputHz: number): number {
  const b = hamBandOf(outputHz);
  if (!b) return 0;
  const mhz = outputHz / 1e6;
  switch (b.id) {
    case "6m":
      return -1_000_000;
    case "2m":
      if (mhz >= 147.0) return 600_000;
      return -600_000; // 145.2–145.5 and 146.0–147.0 outputs
    case "1.25m":
      return -1_600_000;
    case "70cm":
      return mhz < 445.0 ? 5_000_000 : -5_000_000;
    case "33cm":
      return -25_000_000;
    case "23cm":
      return -12_000_000;
    default:
      return 0;
  }
}
