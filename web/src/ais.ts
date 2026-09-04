// AIS vessel-track types — mirror of the server's `VesselsResponse` /
// `ais::tracker::Snapshot`.

export interface Vessel {
  mmsi: number;
  name: string | null;
  callsign: string | null;
  ship_type: number | null;
  ship_type_label: string | null;
  imo: number | null;
  nav_status: number | null;
  nav_status_label: string | null;
  class_b: boolean;
  aid: boolean;
  lat: number | null;
  lon: number | null;
  sog_kt: number | null;
  cog_deg: number | null;
  heading_deg: number | null;
  length_m: number | null;
  beam_m: number | null;
  draught_m: number | null;
  destination: string | null;
  rssi_dbfs: number | null;
  messages: number;
  age_s: number;
  pos_age_s: number | null;
  distance_nm: number | null;
  bearing_deg: number | null;
  trail: [number, number][];
}

export interface AisSnapshot {
  time: string;
  mode: string;
  running: boolean;
  receiver: [number, number] | null;
  messages: number;
  message_rate: number;
  vessel_count: number;
  with_position: number;
  vessels: Vessel[];
}
