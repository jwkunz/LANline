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
  sample_overruns: number;
  pipeline_latency_ms: number | null;
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
  clients: number;
  time: string;
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
