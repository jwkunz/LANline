// APRS station-track snapshot (GET /api/v1/aprs/stations) + a rough symbol
// glyph. The APRS symbol set is a 2-char code (table + code); rendering the
// real icon sheet is a lot of asset weight, so map the common ones to an
// emoji and fall back to the raw code char.

export interface AprsStation {
  call: string;
  lat: number | null;
  lon: number | null;
  course_deg: number | null;
  speed_kt: number | null;
  altitude_ft: number | null;
  symbol: string | null; // table char + code char, e.g. "/>"
  comment: string;
  last_message: string | null;
  path: string[];
  rssi_dbfs: number | null;
  packets: number;
  age_s: number;
  pos_age_s: number | null;
  distance_km: number | null;
  bearing_deg: number | null;
  trail: [number, number][];
}

export interface AprsSnapshot {
  time: string;
  mode: string;
  running: boolean;
  receiver: [number, number] | null;
  packets: number;
  packet_rate: number;
  station_count: number;
  with_position: number;
  stations: AprsStation[];
}

/** The primary-table (`/`) symbol codes worth a distinct glyph. */
const PRIMARY: Record<string, string> = {
  "!": "🚓",
  "#": "📡",
  "$": "📞",
  "&": "🌐",
  "-": "🏠",
  ">": "🚗",
  "<": "🏍️",
  k: "🚚",
  u: "🚛",
  v: "🚐",
  s: "🚢",
  Y: "⛵",
  "'": "✈️",
  "(": "🚁",
  b: "🚴",
  "[": "🚶",
  O: "🎈",
  "_": "🌦️",
  "*": "❄️",
  ";": "⛺",
  "\\": "📍",
  R: "🚙",
  j: "🚙",
  I: "🛰️",
};

export function aprsSymbolGlyph(sym: string | null): string {
  if (!sym || sym.length < 2) return "📍";
  const code = sym[1]!;
  return PRIMARY[code] ?? "📍";
}
