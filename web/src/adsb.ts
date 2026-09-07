// ADS-B track types (mirror of the server's `AircraftResponse` /
// `adsb::Snapshot`) plus the equirectangular projection the radar scope uses.

export interface Aircraft {
  icao: string;
  callsign: string | null;
  category: string | null;
  altitude_ft: number | null;
  lat: number | null;
  lon: number | null;
  ground_speed_kt: number | null;
  track_deg: number | null;
  vertical_rate_fpm: number | null;
  rssi_dbfs: number | null;
  messages: number;
  age_s: number;
  pos_age_s: number | null;
  distance_nm: number | null;
  bearing_deg: number | null;
  trail: [number, number][]; // [lat, lon]
}

/** Internet enrichment for one contact — `GET /api/v1/adsb/flight/{icao}`
 *  (mirror of the server's `flight::FlightInfo`). Every field is optional;
 *  `available` is false when nothing came back and `reason` says why
 *  (`disabled` | `offline` | `unknown` | `pending`). */
export interface FlightInfo {
  available: boolean;
  reason?: string | null;
  registration?: string | null;
  aircraft_type?: string | null;
  icao_type?: string | null;
  manufacturer?: string | null;
  owner?: string | null;
  owner_country?: string | null;
  photo_thumb_url?: string | null;
  airline?: string | null;
  route?: {
    origin: Airport;
    destination: Airport;
  } | null;
}

export interface Airport {
  name?: string | null;
  city?: string | null;
  iata?: string | null;
  icao?: string | null;
}

export interface AdsbSnapshot {
  time: string;
  mode: string;
  running: boolean;
  receiver: [number, number] | null;
  messages: number;
  message_rate: number;
  aircraft_count: number;
  with_position: number;
  aircraft: Aircraft[];
}

/** Nautical miles north / east of a reference point (small-angle flat earth —
 *  fine for a few hundred NM). */
export function offsetNm(
  refLat: number,
  refLon: number,
  lat: number,
  lon: number,
): { north: number; east: number } {
  const north = (lat - refLat) * 60;
  const east = (lon - refLon) * 60 * Math.cos((refLat * Math.PI) / 180);
  return { north, east };
}

/** Flight level label, e.g. 34000 -> "FL340", 2500 -> "2,500 ft". */
export function altLabel(ft: number | null): string {
  if (ft == null) return "—";
  if (ft >= 18000) return "FL" + String(Math.round(ft / 100)).padStart(3, "0");
  return ft.toLocaleString() + " ft";
}
