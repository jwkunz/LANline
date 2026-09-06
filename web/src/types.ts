// TypeScript mirrors of the server DTOs in server/src/model.rs. Only the
// fields the phase-1a client reads are declared.

export interface Ports {
  c2: number;
  audio_out: number;
  audio_in: number;
}

export interface SelectedDevice {
  id: string;
  driver: string;
  label: string;
  tx_capable: boolean;
}

export interface ServerInfo {
  server_id: string;
  protocol_version: number;
  version: string;
  hostname: string;
  time: string;
  ports: Ports;
  capabilities: string[];
  selected_device: SelectedDevice | null;
}

export interface HealthResponse {
  status: string;
  uptime_s: number;
  version: string;
}

export interface CreateSessionResponse {
  session_id: string;
  token: string;
  heartbeat_interval_s: number;
  expires_in_s: number;
}

export interface AudioConfig {
  sample_rate_hz: number;
  channels: number;
  opus_bitrate_bps: number;
  frame_ms: number;
}

export interface RadioConfig {
  enabled: boolean;
  running: boolean;
  mode: string;
  frequency_hz: number;
  tuner: Record<string, unknown>;
  mode_params: Record<string, number>;
  audio: AudioConfig;
}

export interface DspStatus {
  rssi_dbfs: number | null;
  snr_db: number | null;
  squelch_open: boolean;
  audio_level_dbfs: number | null;
  /** Configured CTCSS tone (Hz) while it's currently detected on-channel;
   *  null when CTCSS is off or the tone isn't present. FM modes only. */
  ctcss_tone_hz?: number | null;
  /** Strongest standard CTCSS tone the receiver sees (Hz), independent of the
   *  configured one — for identifying an unknown repeater. null if none. */
  ctcss_scan_hz?: number | null;
  /** Configured DCS code (octal digits as decimal) while decoded on-channel;
   *  null when DCS is off / unlocked. */
  dcs_code?: number | null;
  sample_overruns: number;
  pipeline_latency_ms: number | null;
  tx_keyed: boolean;
}

export interface AudioRecInfo {
  path: string;
  filename: string;
  bytes: number;
  secs: number;
  sample_rate_hz: number;
  mode: string;
  frequency_hz: number;
}

export interface TxLogEntry {
  keyed_at: string;
  client: string;
  mode: string;
  tx_frequency_hz: number;
  offset_hz: number;
  tone_hz: number;
  gain_db: number;
  released_at: string | null;
  duration_ms: number | null;
}

export interface RadioStatus {
  running: boolean;
  mode: string;
  frequency_hz: number;
  device_status: string;
  dsp: DspStatus;
  audio: {
    encoder: string;
    bitrate_bps: number;
    frames_sent: number;
    sample_rate_hz: number;
    channels: number;
  };
  recording: { active: boolean; last: AudioRecInfo | null };
  scan: {
    active: boolean;
    parked: boolean;
    frequency_hz: number | null;
    label: string | null;
    index: number;
    total: number;
  };
  clients: number;
  time: string;
}

export interface Range {
  min: number;
  max: number;
  step: number;
}

export interface DeviceSummary {
  id: string;
  driver: string;
  label: string;
  serial: string;
  soapy_args: string;
  tx_capable: boolean;
  available: boolean;
}

export interface DeviceInfo {
  id: string;
  driver: string;
  label: string;
  serial: string;
  status: string;
  tx_capable: boolean;
  rx: {
    channels: number;
    antennas: string[];
    frequency_ranges_hz: Range[];
    sample_rate_ranges_hz: Range[];
    bandwidth_ranges_hz: Range[];
    gain_elements: { name: string; range_db: Range }[];
    overall_gain_range_db: Range;
    has_agc: boolean;
    has_dc_offset_mode: boolean;
    has_iq_balance_mode: boolean;
    has_frequency_correction: boolean;
    sensors: string[];
    setting_info: unknown[];
  };
}

export interface AnalysisStatus {
  running: boolean;
  center_hz: number;
  span_hz: number;
  n_bins: number;
  seq: number;
  rows_held: number;
  recording: boolean;
  last_recording: {
    filename: string;
    path: string;
    bytes: number;
    secs: number;
    sample_rate_hz: number;
    center_hz: number;
  } | null;
}

/** `radio.tuner` — a loose bag the Radio Options panel reads and writes. */
export interface TunerConfig {
  device_id: string | null;
  channel: number;
  antenna: string | null;
  sample_rate_hz: number;
  bandwidth_hz: number | null;
  lo_offset_hz: number;
  gain_mode: "manual" | "agc";
  gain_db: number | null;
  gain_elements_db: Record<string, number>;
  freq_correction_ppm: number;
  dc_offset_correction: boolean;
  iq_balance_correction: boolean;
  device_settings: Record<string, string>;
}

export interface ModeInfo {
  id: string;
  name: string;
  tx_capable: boolean;
  params: Record<string, unknown>;
}

export interface AudioStateResponse {
  state: string;
  ice_state: string;
  dtls_state: string;
  packets_sent: number;
  bytes_sent: number;
}

export interface SdpMessage {
  sdp: string;
  type: string;
}

export interface ApiErrorBody {
  error: { code: string; message: string; details?: unknown };
}
