import "./style.css";
import { ApiError, Client, normalizeBase } from "./api";
import { openRadioOptions, radioOptionsOpen } from "./radio-options";
import { AnalysisView } from "./analysis";
import { AudioSession, type AudioState } from "./audio";
import { nativeDiscovery, serverHost } from "./discovery";
import { haversineMi, parseLatLon, type Located } from "./geo";
import { altLabel, offsetNm, type AdsbSnapshot, type FlightInfo } from "./adsb";
import { type AisSnapshot, type VesselInfo } from "./ais";
import { aprsSymbolGlyph, parseAprsMessages, type AprsSnapshot } from "./aprs";
import { loadNwrStations, nearestNwr, NWR_CHANNELS, type NwrStation } from "./nwr";
import {
  FM_MAX_HZ,
  FM_MIN_HZ,
  loadFmStations,
  nearestFm,
  snapFm,
  stepFm,
  type FmStation,
} from "./fm";
import {
  AM_MAX_HZ,
  AM_MIN_HZ,
  loadAmStations,
  nearestAm,
  snapAm,
  stepAm,
  type AmStation,
} from "./am";
import {
  APT_MAX_HZ,
  APT_MIN_HZ,
  APT_SATELLITES,
  loadAptTle,
  nextPasses,
  type AptImage,
  type AptPass,
  type AptStatus,
} from "./apt";
import { FRS_CHANNELS, FRS_DEFAULT_FREQ_HZ, frsChannelAt, stepFrsChannel } from "./frs";
import {
  CTCSS_TONES,
  DCS_CODES,
  HAM_BANDS,
  HAM_DEFAULT_BAND,
  conventionalOffsetHz,
  hamBand,
  hamSegmentAt,
  hamSimplexChannels,
  loadRepeaters,
  offsetLabel,
  parseRepeaterShorthand,
  type Repeater,
} from "./ham";
import type {
  AudioStateResponse,
  CreateSessionResponse,
  DeviceInfo,
  DeviceSummary,
  ModeInfo,
  RadioConfig,
  RadioStatus,
  TranscriptEntry,
  ServerInfo,
  TxLogEntry,
} from "./types";

const HOST_KEY = "lanline.host";
const LOC_KEY = "lanline.loc";
const freqKey = (mode: string) => `lanline.freq.${mode}`;

type Phase = "disconnected" | "connecting" | "connected" | "error";

interface ModeMeta {
  label: string;
  icon: string;
  band: string;
  defaultFreqHz: number;
  loOffsetHz: number;
  wantSampleRateHz: number;
}

const MODE_META: Record<string, ModeMeta> = {
  nbfm: {
    label: "NOAA Weather",
    icon: "⛅",
    band: "162 MHz",
    defaultFreqHz: 162_550_000,
    loOffsetHz: 250_000,
    wantSampleRateHz: 2_000_000,
  },
  wbfm: {
    label: "FM Broadcast",
    icon: "📻",
    band: "88–108 MHz",
    defaultFreqHz: 98_500_000,
    loOffsetHz: 250_000,
    wantSampleRateHz: 4_000_000,
  },
  am: {
    label: "AM Radio",
    icon: "🗼",
    band: "520–1710 kHz",
    defaultFreqHz: 1_000_000,
    loOffsetHz: 25_000,
    wantSampleRateHz: 1_000_000,
  },
  analysis: {
    label: "Analysis",
    icon: "✳️",
    band: "spectrum",
    defaultFreqHz: 100_000_000,
    loOffsetHz: 0,
    wantSampleRateHz: 2_000_000,
  },
  adsb: {
    label: "ADS-B",
    icon: "✈️",
    band: "1090 MHz",
    defaultFreqHz: 1_090_000_000,
    loOffsetHz: 0,
    wantSampleRateHz: 2_000_000,
  },
  aprs: {
    label: "APRS",
    icon: "📍",
    band: "144.39 MHz",
    defaultFreqHz: 144_390_000,
    loOffsetHz: 250_000,
    wantSampleRateHz: 2_000_000,
  },
  ais: {
    label: "AIS",
    icon: "🚢",
    band: "162 MHz",
    defaultFreqHz: 162_000_000,
    loOffsetHz: 0,
    wantSampleRateHz: 2_000_000,
  },
  apt: {
    label: "NOAA APT",
    icon: "🛰️",
    band: "137 MHz",
    defaultFreqHz: 137_100_000,
    loOffsetHz: 25_000,
    wantSampleRateHz: 2_000_000,
  },
  frs: {
    label: "FRS",
    icon: "🎙️",
    band: "462/467 MHz",
    defaultFreqHz: FRS_DEFAULT_FREQ_HZ,
    loOffsetHz: 250_000,
    wantSampleRateHz: 2_000_000,
  },
  ham: {
    label: "Amateur FM",
    icon: "📡",
    band: "VHF/UHF",
    defaultFreqHz: 146_520_000,
    loOffsetHz: 250_000,
    wantSampleRateHz: 2_000_000,
  },
  debug_tone: {
    label: "Debug Tone",
    icon: "🔊",
    band: "—",
    defaultFreqHz: 162_550_000,
    loOffsetHz: 250_000,
    wantSampleRateHz: 2_000_000,
  },
};

type NearNwr = NwrStation & { distance_mi: number };
type NearFm = FmStation & { distance_mi: number };
type NearAm = AmStation & { distance_mi: number };
type NearRepeater = Repeater & { distance_mi: number | null };

const RP_MANUAL_KEY = "lanline.repeaters.manual";

function readManualRepeaters(): Repeater[] {
  try {
    const v = JSON.parse(localStorage.getItem(RP_MANUAL_KEY) ?? "[]");
    return Array.isArray(v) ? (v as Repeater[]) : [];
  } catch {
    return [];
  }
}

interface State {
  phase: Phase;
  base: string;
  error: string | null;
  client: Client | null;
  session: CreateSessionResponse | null;
  server: ServerInfo | null;
  device: DeviceInfo | null;
  devices: DeviceSummary[];
  modes: ModeInfo[];
  radio: RadioConfig | null;
  status: RadioStatus | null;
  switching: boolean;
  audio: AudioSession | null;
  audioState: AudioState;
  audioDetail: string | null;
  audioStats: AudioStateResponse | null;
  loc: Located | null;
  locNote: string | null;
  nwrNearby: NearNwr[];
  fmNearby: NearFm[];
  amNearby: NearAm[];
  aptPasses: Record<string, AptPass[]> | null;
  bandTab: "nearby" | "manual";
  hamBand: string;
  hamView: "simplex" | "repeater";
  repeaters: NearRepeater[];
  manualRepeaters: Repeater[];
  activeRepeater: Repeater | null;
  rpAddOpen: boolean;
  txLog: TxLogEntry[];
  seeking: boolean;
  pttHeld: boolean;
  adsb: AdsbSnapshot | null;
  ais: AisSnapshot | null;
  aprs: AprsSnapshot | null;
  aprsPackets: string[];
  transcript: TranscriptEntry[];
  apt: AptStatus | null;
  scopeSel: string | null;
  /** Internet enrichment for ADS-B contacts, keyed by ICAO hex. `"loading"`
   *  while the request is in flight. Cleared on mode switch. */
  acInfo: Record<string, FlightInfo | "loading">;
  /** Same, for AIS contacts, keyed by MMSI. */
  vesselInfo: Record<string, VesselInfo | "loading">;
  scopeRangeNm: number | "auto";
  log: string[];
}

const state: State = {
  phase: "disconnected",
  base: "",
  error: null,
  client: null,
  session: null,
  server: null,
  device: null,
  devices: [],
  modes: [],
  radio: null,
  status: null,
  switching: false,
  audio: null,
  audioState: "idle",
  audioDetail: null,
  audioStats: null,
  loc: readSavedLoc(),
  locNote: null,
  nwrNearby: [],
  fmNearby: [],
  amNearby: [],
  aptPasses: null,
  bandTab: "nearby",
  hamBand: localStorage.getItem("lanline.hamBand") ?? HAM_DEFAULT_BAND,
  hamView: localStorage.getItem("lanline.hamView") === "repeater" ? "repeater" : "simplex",
  repeaters: [],
  manualRepeaters: readManualRepeaters(),
  activeRepeater: null,
  rpAddOpen: false,
  txLog: [],
  seeking: false,
  pttHeld: false,
  adsb: null,
  ais: null,
  aprs: null,
  aprsPackets: [],
  transcript: [],
  apt: null,
  scopeSel: null,
  acInfo: {},
  vesselInfo: {},
  scopeRangeNm: "auto",
  log: [],
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let heartbeatTimer: number | undefined;
let pollTimer: number | undefined;
let pollFails = 0;

/** Last decoded APT raster (rendering cache, not reactive state — see
 *  `refreshAptImage`/`drawAptImage`, mirroring `scopePlot` below). */
let aptImg: AptImage | null = null;
let aptImageFetchedAt = 0;
let analysisView: AnalysisView | null = null;

function readSavedLoc(): Located | null {
  return parseLatLon(localStorage.getItem(LOC_KEY) ?? "");
}

// --- shell -----------------------------------------------------------------

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
  <div class="app-head">
    <h1>LANline <span class="sub" id="app-sub">SDR on your LAN</span></h1>
    <a id="fleet-link" class="fleet-link secondary" hidden>&#8676;&nbsp;Fleet</a>
  </div>
  <p class="tagline">Discover a radio server on the LAN, pick a waveform, and listen.</p>
  <section class="card">
    <h2>Connection</h2>
    <div id="discovered" class="discovered" hidden></div>
    <div class="connect-row">
      <input id="host" type="text" spellcheck="false" autocapitalize="off"
             placeholder="server host, e.g. lanline.local:8730" />
      <button id="action">Connect</button>
    </div>
    <p id="conn-status" class="note" style="margin:12px 0 0"></p>
  </section>
  <div id="panels"></div>
`;

const hostInput = app.querySelector<HTMLInputElement>("#host")!;
const actionBtn = app.querySelector<HTMLButtonElement>("#action")!;
const connStatus = app.querySelector<HTMLParagraphElement>("#conn-status")!;
const panels = app.querySelector<HTMLDivElement>("#panels")!;
const discoveredBox = app.querySelector<HTMLDivElement>("#discovered")!;
const appSub = app.querySelector<HTMLSpanElement>("#app-sub")!;
const fleetLink = app.querySelector<HTMLAnchorElement>("#fleet-link")!;

hostInput.value = localStorage.getItem(HOST_KEY) ?? "";
actionBtn.addEventListener("click", onAction);
hostInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") onAction();
});

// LAN discovery (Android wrapper only).
const pollDiscovered = nativeDiscovery();
const NATIVE = pollDiscovered !== null;
console.info(`lanline: native=${NATIVE} savedHost=${JSON.stringify(hostInput.value)}`);

const idle = (): boolean =>
  state.phase === "disconnected" || state.phase === "error";

let savedHostTried = false;
function trySavedHost(): void {
  if (savedHostTried || !idle() || !hostInput.value.trim()) return;
  savedHostTried = true;
  onAction();
}

/** If this page was served *by* a LANline server, its API is the same origin —
 *  connect there with nothing to type. Returns the base URL or null. */
async function probeOrigin(): Promise<string | null> {
  if (location.protocol !== "http:" && location.protocol !== "https:") return null;
  const base = `${location.protocol}//${location.host}`;
  try {
    const ctl = new AbortController();
    const timer = window.setTimeout(() => ctl.abort(), 2500);
    const res = await fetch(base + "/health", {
      signal: ctl.signal,
      headers: { accept: "application/json" },
    });
    window.clearTimeout(timer);
    if (!res.ok) return null;
    const body = (await res.json().catch(() => null)) as { status?: string } | null;
    return body?.status === "ok" ? base : null;
  } catch {
    return null; // not served by a LANline server (dev server, static host, file://)
  }
}

void (async () => {
  const origin = await probeOrigin();
  if (origin && idle()) {
    savedHostTried = true; // don't also race the saved host
    hostInput.value = origin;
    console.info(`lanline: served by ${origin} — auto-connecting`);
    void connect(origin);
    return;
  }
  window.setTimeout(trySavedHost, 150);
})();

if (pollDiscovered) {
  let discoveryAutoTried = false;
  let dbgCount = 0;
  const renderDiscovered = (): void => {
    if (state.phase === "connected" || state.phase === "connecting") {
      discoveredBox.hidden = true;
      return;
    }
    const servers = pollDiscovered().sort((a, b) =>
      a.hostname.localeCompare(b.hostname),
    );
    if (servers.length === 0) {
      discoveredBox.hidden = true;
      return;
    }
    if (dbgCount++ < 6)
      console.info(`lanline: discovered ${servers.length} ${JSON.stringify(servers.map((s) => s.hostname))}`);
    if (!discoveryAutoTried && idle()) {
      discoveryAutoTried = true;
      hostInput.value = serverHost(servers[0]!);
      state.error = null;
      onAction();
      return;
    }
    discoveredBox.hidden = false;
    discoveredBox.innerHTML =
      `<div class="note" style="margin-bottom:6px">Discovered on LAN</div>` +
      servers
        .map((s) => {
          const dev = s.device ? ` · ${esc(s.device)}` : "";
          return `<button class="secondary disc-item" data-host="${esc(serverHost(s))}">${esc(s.hostname)}${dev}</button>`;
        })
        .join("");
    discoveredBox
      .querySelectorAll<HTMLButtonElement>(".disc-item")
      .forEach((btn) => {
        btn.addEventListener("click", () => {
          hostInput.value = btn.dataset.host ?? "";
          if (idle()) onAction();
        });
      });
  };
  renderDiscovered();
  window.setInterval(renderDiscovered, 2000);
}

// --- connection --------------------------------------------------------

function onAction(): void {
  if (state.phase === "connected" || state.phase === "connecting") {
    void disconnect();
  } else {
    void connect(hostInput.value);
  }
}

// A WebView fetch can wedge (e.g. the network stack gets suspended mid-request
// during an app-launch activity transition) with nothing to catch it — so
// every connect attempt gets a generation number and an AbortController: a
// newer attempt or an explicit cancel (the Connect/Disconnect button stays
// live while "Connecting…") invalidates the old one instead of leaving it to
// hang forever with no way out but restarting the app.
let connectSeq = 0;
let connectAbort: AbortController | null = null;

/** The mic stream behind FRS's PTT, when granted — muted (`track.enabled =
 *  false`) except while actually held (see `pttDown`/`pttUp`), released
 *  whenever audio tears down. `null` in every other mode: only `frs` ever
 *  requests it (see `acquireMicIfNeeded`), so nothing else prompts for mic
 *  permission at all. */
let micStream: MediaStream | null = null;

async function connect(hostRaw: string): Promise<void> {
  if (!hostRaw.trim()) {
    setState({ phase: "error", error: "enter a server host first" });
    return;
  }
  const base = normalizeBase(hostRaw);
  localStorage.setItem(HOST_KEY, hostRaw.trim());

  const mySeq = ++connectSeq;
  connectAbort?.abort();
  const abort = new AbortController();
  connectAbort = abort;
  const stale = () => mySeq !== connectSeq;

  setState({ phase: "connecting", base, error: null });

  console.info(`lanline: connecting ${base}`);
  const client = new Client(base);
  try {
    await client.health(abort.signal);
    if (stale()) return;
    const server = await client.server(abort.signal);
    if (stale()) return;
    const modes = await client.modes(abort.signal).catch(() => [] as ModeInfo[]);
    const device = await client.device(abort.signal).catch(() => null);
    const devices = await client.devices(abort.signal).catch(() => [] as DeviceSummary[]);
    if (stale()) return;
    console.info(`lanline: rest ok (${server.hostname}, ${modes.length} modes, device=${!!device})`);
    const session = await client.createSession(
      { name: NATIVE ? "android" : "web", user_agent: navigator.userAgent, capabilities: ["webrtc-recv"] },
      abort.signal,
    );
    if (stale()) return;
    client.setToken(session.token);
    const [radio, status] = await Promise.all([
      client.radio(abort.signal),
      client.radioStatus(abort.signal),
    ]);
    if (stale()) return;

    state.client = client;
    state.session = session;
    state.modes = modes;
    state.device = device;
    state.devices = devices;

    const audio = new AudioSession(client, session.session_id);
    audio.element.setAttribute("hidden", "");
    document.body.appendChild(audio.element);
    audio.onstate = (s, detail) => {
      state.audioState = s;
      state.audioDetail = detail ?? null;
      if (s === "playing") logLine("audio playing");
      else if (s === "failed") logLine(`audio failed — ${detail ?? "unknown"}`);
      render();
    };
    state.audio = audio;
    state.audioState = "idle";
    state.audioStats = null;

    setState({ phase: "connected", server, radio, status, error: null });
    logLine(
      `connected to ${server.hostname} · session ${session.session_id.slice(0, 8)}`,
    );
    // Tell the Android wrapper which radio we landed on, so its native radio
    // chooser doesn't pop over an already-connected page and its toolbar shows
    // the right label.
    try {
      (
        window as unknown as {
          LanlineNative?: { onConnected?: (base: string, label: string, fleetUrl: string) => void };
        }
      ).LanlineNative?.onConnected?.(base, server.instance_label ?? "", server.fleet_url ?? "");
    } catch {
      /* not in the wrapper */
    }
    startTimers();

    if (state.loc) void refreshStations();
    if (NATIVE) void autoListen();
  } catch (e) {
    if (stale()) return; // superseded by a newer attempt or a manual cancel
    const err = e as ApiError;
    console.warn(`lanline: connect failed — ${err.code}: ${err.message}`);
    state.client = null;
    state.session = null;
    setState({ phase: "error", error: `${err.code}: ${err.message}` });
    logLine(`connect failed — ${err.message}`);
  } finally {
    if (connectAbort === abort) connectAbort = null;
  }
}

async function autoListen(): Promise<void> {
  if (!state.client || !state.audio) return;
  try {
    if (!state.radio?.running) {
      setState({ radio: await state.client.startRadio() });
      logLine(`radio started (${state.radio?.mode})`);
    }
  } catch (e) {
    logLine(`auto start-radio failed — ${(e as ApiError).message}`);
  }
  const m = state.radio?.mode;
  if (m === "adsb" || m === "ais" || m === "aprs" || m === "apt") return; // data-only modes, no audio
  try {
    if (state.audioState === "idle") await startAudio();
  } catch (e) {
    logLine(`auto play failed — ${(e as Error).message}`);
  }
}

/** Mic permission is requested only for the PTT-capable modes (`frs`, `ham`),
 *  and only when the server actually offers `ptt` — no point prompting for
 *  a mic that has nowhere to go. Failure (denied, no device) degrades to
 *  receive-only, not a hard error: normal listening still works, PTT just
 *  won't have real audio behind it (silence — see `TxAudioSource`). */
async function acquireMicIfNeeded(): Promise<MediaStream | null> {
  const m = state.radio?.mode;
  if ((m !== "frs" && m !== "ham") || !state.server?.capabilities.includes("ptt")) return null;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1 } });
    const track = stream.getAudioTracks()[0];
    logLine(`mic granted — track: ${track?.label || "(no label)"}, state: ${track?.readyState}`);
    return stream;
  } catch (e) {
    logLine(`mic unavailable — PTT will transmit silence (${(e as Error).message})`);
    return null;
  }
}

/** The one place that actually starts playback — acquires (or reuses) the
 *  PTT mic stream first so the offer negotiates bidirectional when it
 *  should, muted until actually held. */
async function startAudio(): Promise<void> {
  if (!state.audio) return;
  if (!micStream) micStream = await acquireMicIfNeeded();
  if (micStream) micStream.getAudioTracks().forEach((t) => (t.enabled = false));
  await state.audio.start(micStream);
}

async function disconnect(): Promise<void> {
  connectSeq++; // invalidate any in-flight connect() attempt
  connectAbort?.abort();
  connectAbort = null;
  stopTimers();
  await teardownAudio();
  const client = state.client;
  const session = state.session;
  state.client = null;
  state.session = null;
  if (client && session) {
    try {
      await client.deleteSession(session.session_id);
    } catch {
      /* best effort */
    }
  }
  setState({
    phase: "disconnected",
    server: null,
    device: null,
    devices: [],
    radio: null,
    status: null,
    error: null,
  });
  logLine("disconnected");
}

async function teardownAudio(): Promise<void> {
  const audio = state.audio;
  state.audio = null;
  state.audioState = "idle";
  state.audioDetail = null;
  state.audioStats = null;
  if (audio) {
    await audio.stop().catch(() => {});
    audio.element.remove();
  }
  if (micStream) {
    micStream.getTracks().forEach((t) => t.stop());
    micStream = null;
  }
}

// --- radio / mode / tuning -------------------------------------------

async function toggleRadio(): Promise<void> {
  if (!state.client || !state.radio) return;
  const wasRunning = state.radio.running;
  try {
    const radio = wasRunning
      ? await state.client.stopRadio()
      : await state.client.startRadio();
    setState({ radio });
    logLine(`radio ${radio.running ? "started" : "stopped"}`);
  } catch (e) {
    logLine(`radio ${wasRunning ? "stop" : "start"} failed — ${(e as ApiError).message}`);
  }
}

async function openRadioOptionsPanel(): Promise<void> {
  if (!state.client || !state.radio) return;
  await openRadioOptions(state.client, state.radio, (updated) => {
    setState({ radio: updated });
    logLine("radio options applied");
  });
}

async function toggleAudio(): Promise<void> {
  const audio = state.audio;
  if (!audio) return;
  if (audio.state === "playing" || audio.state === "connecting") {
    await audio.stop().catch(() => {});
    state.audioStats = null;
    if (micStream) {
      micStream.getTracks().forEach((t) => t.stop());
      micStream = null;
    }
    render();
  } else {
    try {
      await startAudio();
    } catch (e) {
      logLine(`audio start failed — ${(e as Error).message}`);
    }
  }
}

async function toggleAudioRecord(): Promise<void> {
  if (!state.client) return;
  const active = state.status?.recording.active ?? false;
  try {
    if (active) {
      const r = (await state.client.recordAudio("stop")) as { last?: { secs: number } };
      logLine(`recording stopped${r.last ? ` (${r.last.secs.toFixed(1)}s)` : ""}`);
    } else {
      await state.client.recordAudio("start", 300);
      logLine("recording started (48 kHz WAV, 5 min cap)");
    }
  } catch (e) {
    logLine(`recording — ${(e as ApiError).message}`);
  }
  // Reflect the new state now rather than waiting for the next poll.
  try {
    setState({ status: await state.client.radioStatus() });
  } catch {
    /* next poll will catch up */
  }
}

function toggleMute(): void {
  if (state.audio) {
    state.audio.setMuted(!state.audio.muted);
    render();
  }
}

/** A sample rate the device can do, at or below `want`. Handles both a
 *  continuous range (Pluto: 65 kHz–61 MHz — use `want` exactly) and a set of
 *  discrete rates (HackRF: 1–20 Msps in 1 MHz steps — nearest ≤ want). */
function pickSampleRateFor(dev: DeviceInfo | null, wantHz: number): number | null {
  const ranges = dev?.rx.sample_rate_ranges_hz ?? [];
  if (ranges.length === 0) return null;
  for (const r of ranges) {
    if (r.max > r.min && wantHz >= r.min && wantHz <= r.max) return Math.round(wantHz);
  }
  const pts = ranges
    .flatMap((r) => (r.max > r.min ? [r.min, r.max] : [r.min]))
    .filter((v) => v > 0);
  const under = pts.filter((v) => v <= wantHz + 1);
  return under.length ? Math.max(...under) : Math.min(...pts);
}
const pickSampleRate = (wantHz: number) => pickSampleRateFor(state.device, wantHz);

/** SDR devices worth showing in the picker (SoapySDR also enumerates sound
 *  cards via its `audio` module — never those). */
function sdrDevices(): DeviceSummary[] {
  return state.devices.filter((d) => d.driver !== "audio" && d.available);
}

/** Does a mode's home frequency fall in the selected device's tuning range? */
function modeInDeviceRange(id: string): boolean {
  const hz = MODE_META[id]?.defaultFreqHz;
  const ranges = state.device?.rx.frequency_ranges_hz;
  if (hz == null || !ranges?.length) return true;
  return ranges.some((r) => hz >= r.min && hz <= r.max);
}

async function switchDevice(id: string): Promise<void> {
  if (!state.client || state.switching || state.device?.id === id) return;
  setState({ switching: true });
  try {
    if (state.radio?.running) await state.client.stopRadio();
    const info = await state.client.selectDevice(id);
    // Drop tuner overrides that belonged to the old radio (its antenna
    // name, its gain elements) so they don't fail validation on the new one,
    // then re-pick a sample rate this device actually supports.
    const want = MODE_META[state.radio?.mode ?? "nbfm"]?.wantSampleRateHz ?? 2_000_000;
    const sr = pickSampleRateFor(info, want);
    const radio = await state.client.patchRadio({
      tuner: {
        antenna: null,
        gain_elements_db: {},
        gain_db: null,
        gain_mode: "manual",
        bandwidth_hz: null,
        ...(sr ? { sample_rate_hz: sr } : {}),
      },
    });
    const devices = await state.client.devices().catch(() => state.devices);
    setState({ device: info, devices, radio });
    logLine(`radio → ${info.label}`);
  } catch (e) {
    logLine(`device switch failed — ${(e as ApiError).message}`);
    try {
      setState({ device: await state.client.device(), radio: await state.client.radio() });
    } catch {
      /* leave stale */
    }
  } finally {
    setState({ switching: false });
  }
}

async function switchMode(id: string): Promise<void> {
  if (!state.client || state.switching || state.radio?.mode === id) return;
  const meta = MODE_META[id];
  setState({ switching: true });

  const fixedFreq = id === "adsb" || id === "ais";
  const patch: Record<string, unknown> = { mode: id };
  if (id !== "debug_tone") {
    const last = Number(localStorage.getItem(freqKey(id)));
    patch.frequency_hz = fixedFreq
      ? meta!.defaultFreqHz
      : Number.isFinite(last) && last > 0
        ? last
        : id === "ham"
          ? hamBand(state.hamBand).defaultHz
          : meta?.defaultFreqHz;
    const sr = pickSampleRate(meta?.wantSampleRateHz ?? 2_000_000);
    const tuner: Record<string, unknown> = { lo_offset_hz: meta?.loOffsetHz ?? 250_000 };
    if (sr) tuner.sample_rate_hz = sr;
    patch.tuner = tuner;
  }
  if (id === "adsb") {
    patch.mode_params = {
      reference_lat: state.loc?.lat ?? 0,
      reference_lon: state.loc?.lon ?? 0,
      max_range_nm: 250,
      trail_seconds: 120,
      forget_seconds: 60,
      fix_errors: 1,
    };
  } else if (id === "ais") {
    patch.mode_params = {
      reference_lat: state.loc?.lat ?? 0,
      reference_lon: state.loc?.lon ?? 0,
      max_range_nm: 60,
      trail_seconds: 600,
      forget_seconds: 900,
    };
  } else if (id === "aprs") {
    patch.mode_params = {
      reference_lat: state.loc?.lat ?? 0,
      reference_lon: state.loc?.lon ?? 0,
      max_range_km: 300,
      trail_seconds: 1800,
      forget_seconds: 3600,
    };
  }

  aptImg = null;
  aptImageFetchedAt = 0;
  // Release any PTT mic from the mode being left — startAudio() only
  // re-acquires when the *new* mode actually needs one (frs), and a stale
  // stream here would otherwise carry into a mode that has no business
  // being bidirectional.
  if (micStream) {
    micStream.getTracks().forEach((t) => t.stop());
    micStream = null;
  }
  try {
    let radio = await state.client.patchRadio(patch);
    setState({
      radio,
      adsb: null,
      ais: null,
      aprs: null,
      apt: null,
      scopeSel: null,
      acInfo: {},
      vesselInfo: {},
      activeRepeater: null,
    });
    logLine(`mode → ${meta?.label ?? id}`);
    void refreshStations();
    if (id === "frs" || id === "ham") void refreshTxLog();
    if (radio.running || NATIVE) {
      if (!radio.running) radio = await state.client.startRadio();
      setState({ radio });
      if (!fixedFreq && id !== "analysis" && state.audio && state.audioState === "idle")
        void startAudio();
    }
  } catch (e) {
    const err = e as ApiError;
    logLine(`mode switch failed — ${err.message}`);
  } finally {
    setState({ switching: false });
  }
}

/** MHz (4 digits, 1 for wbfm) or kHz (am) label for a frequency in a mode. */
function freqLabel(mode: string, hz: number): string {
  if (mode === "am") return `${(hz / 1e3).toFixed(0)} kHz`;
  if (mode === "frs") {
    const c = frsChannelAt(hz);
    return c ? `Ch ${c.channel}` : `${(hz / 1e6).toFixed(4)} MHz`;
  }
  return `${(hz / 1e6).toFixed(mode === "wbfm" ? 1 : 4)} MHz`;
}

async function tuneFrequency(hz: number, label?: string): Promise<void> {
  if (!state.client || !state.radio) return;
  const freq = Math.round(hz);
  try {
    let radio = await state.client.patchRadio({ frequency_hz: freq });
    localStorage.setItem(freqKey(radio.mode), String(freq));
    if (!radio.running) radio = await state.client.startRadio();
    // Tuning by hand / to a simplex channel means we're no longer parked on
    // the selected repeater.
    const activeRepeater =
      state.activeRepeater && state.activeRepeater.output_hz === freq ? state.activeRepeater : null;
    setState({ radio, activeRepeater });
    if (state.audio && state.audioState === "idle") void startAudio();
    logLine(`tuned ${label ?? freqLabel(radio.mode, freq)}`);
  } catch (e) {
    logLine(`tune failed — ${(e as ApiError).message}`);
  }
}

/** Scan up/down for the next occupied channel. Client-driven: step, wait, read
 *  status, stop on signal. */
async function seek(dir: 1 | -1): Promise<void> {
  if (!state.client || !state.radio || state.seeking) return;
  const mode = state.radio.mode;
  if (mode !== "wbfm" && mode !== "nbfm" && mode !== "am" && mode !== "frs") return;

  // Scan the actual broadcast band (not the wider manual-tune range). `frs`
  // steps by channel number, not frequency — see `stepFrsChannel`.
  const step = mode === "wbfm" ? 200_000 : mode === "am" ? 10_000 : 25_000;
  const lo = mode === "wbfm" ? 87_700_000 : mode === "am" ? 530_000 : 162_400_000;
  const hi = mode === "wbfm" ? 108_100_000 : mode === "am" ? 1_700_000 : 162_550_000;
  // wbfm has no squelch, so seek stops purely on RSSI — keep this strict
  // enough to skip fringe carriers and land on a station you'd actually
  // listen to. (nbfm/frs also require squelch_open, so they can be looser.)
  const rssiGate = mode === "wbfm" ? -40 : mode === "am" ? -55 : mode === "frs" ? -70 : -75;
  const settleMs = mode === "wbfm" || mode === "am" ? 220 : 200;
  const steps = mode === "frs" ? FRS_CHANNELS.length : Math.round((hi - lo) / step) + 1;

  setState({ seeking: true });
  logLine(`seek ${dir > 0 ? "up" : "down"}…`);
  const start = state.radio.frequency_hz;
  let f = start;
  try {
    for (let i = 0; i < steps; i++) {
      if (mode === "frs") {
        f = stepFrsChannel(f, dir);
      } else {
        f += dir * step;
        if (f > hi) f = lo;
        if (f < lo) f = hi;
      }
      if (f === start) break; // wrapped all the way around
      await state.client.patchRadio({ frequency_hz: f });
      await sleep(settleMs);
      const s = await state.client.radioStatus().catch(() => null);
      const rssi = s?.dsp.rssi_dbfs ?? -200;
      const open =
        mode === "nbfm" || mode === "frs" ? (s?.dsp.squelch_open ?? false) && rssi > rssiGate : rssi > rssiGate;
      if (open) break;
    }
    localStorage.setItem(freqKey(mode), String(f));
    const radio = await state.client.radio();
    setState({ radio });
    logLine(`seek → ${freqLabel(mode, radio.frequency_hz)}`);
    void refreshStations();
  } catch (e) {
    logLine(`seek failed — ${(e as ApiError).message}`);
  } finally {
    setState({ seeking: false });
  }
}

/** Build the channel list for a server-side scan of the current mode. Returns
 *  `{channels}` or `{range}` for `client.scan("start", …)`. */
function scanListForMode(): {
  channels?: { frequency_hz: number; label?: string }[];
  range?: { lo_hz: number; hi_hz: number; step_hz: number };
  rssi_gate_dbfs: number;
} {
  const mode = state.radio?.mode;
  if (mode === "frs") {
    return {
      channels: FRS_CHANNELS.map((c) => ({ frequency_hz: c.freq_hz, label: `Ch ${c.channel}` })),
      rssi_gate_dbfs: -70,
    };
  }
  if (mode === "nbfm") {
    const near = state.nwrNearby;
    const channels = near.length
      ? near.map((s) => ({ frequency_hz: s.freq_hz, label: `${s.callsign} · ${s.site}` }))
      : NWR_CHANNELS.map((hz) => ({ frequency_hz: hz, label: `${(hz / 1e6).toFixed(3)} MHz` }));
    return { channels, rssi_gate_dbfs: -75 };
  }
  if (mode === "ham") {
    const band = hamBandContaining(state.radio!.frequency_hz) ?? hamBand(state.hamBand);
    const simplex = hamSimplexChannels(band).map((c) => ({ frequency_hz: c.hz, label: c.name }));
    const rpts = state.repeaters.map((rp) => ({
      frequency_hz: rp.output_hz,
      label: `${rp.call} ${offsetLabel(rp.offset_hz)}`,
    }));
    return { channels: [...simplex, ...rpts], rssi_gate_dbfs: -78 };
  }
  if (mode === "wbfm") {
    return { range: { lo_hz: 87_900_000, hi_hz: 107_900_000, step_hz: 200_000 }, rssi_gate_dbfs: -40 };
  }
  // am
  return { range: { lo_hz: 530_000, hi_hz: 1_700_000, step_hz: 10_000 }, rssi_gate_dbfs: -55 };
}

async function toggleScan(): Promise<void> {
  if (!state.client) return;
  const active = state.status?.scan.active ?? false;
  try {
    if (active) {
      await state.client.scan("stop");
      logLine("scan stopped");
      // Adopt whatever frequency the scan left us on.
      setState({ radio: await state.client.radio() });
    } else {
      const list = scanListForMode();
      const n = await state.client.scan("start", list);
      logLine(`scanning ${n.channels ?? "band"} channels…`);
    }
  } catch (e) {
    logLine(`scan — ${(e as ApiError).message}`);
  }
  try {
    setState({ status: await state.client.radioStatus() });
  } catch {
    /* next poll catches up */
  }
}

// --- push-to-talk (FRS transmit, live mic audio — see docs/architecture.md) -

// Slightly under the server's own hard cap (10s) so the client always
// releases first and shows an accurate "released" state rather than racing
// the server's own auto-unkey.
const PTT_MAX_HOLD_MS = 8_000;
let pttTimer: number | undefined;

/** PTT can be the very first thing to touch audio in a session — e.g. the
 *  user pressed "Start radio" rather than "Play", which never calls
 *  `startAudio()` at all, so no mic was ever requested. Make sure one
 *  actually gets attached right here rather than assuming some earlier
 *  code path already did it. A pre-existing (recvonly) connection can't
 *  retroactively gain a track, so if one's already up, rebuild it with the
 *  mic from the start — a brief RX audio blip, but only the first time PTT
 *  is used in a session. */
async function ensureMicAttached(): Promise<void> {
  if (!state.audio || micStream) return;
  const stream = await acquireMicIfNeeded();
  if (!stream) return; // denied/unavailable — PTT will transmit silence
  micStream = stream;
  micStream.getAudioTracks().forEach((t) => (t.enabled = false));
  await state.audio.stop().catch(() => {});
  await state.audio.start(micStream);
}

async function pttDown(): Promise<void> {
  if (!state.client || state.pttHeld) return;
  await ensureMicAttached();
  setState({ pttHeld: true });
  // Unmute before (not after) keying so the first syllable isn't clipped
  // waiting on the key round-trip.
  micStream?.getAudioTracks().forEach((t) => (t.enabled = true));
  try {
    // ham TX: a selected repeater's split + uplink tone wins; otherwise
    // encode whatever sub-audible squelch the wizard has configured.
    const isHam = state.radio?.mode === "ham";
    const rp =
      isHam && state.activeRepeater?.output_hz === state.radio!.frequency_hz
        ? state.activeRepeater
        : null;
    const mp = state.radio?.mode_params ?? {};
    let keyOpts: Parameters<typeof state.client.keyTx>[0];
    if (rp) {
      keyOpts = { offsetHz: rp.offset_hz, toneHz: rp.tone_hz };
    } else if (isHam && Number(mp.ctcss_hz ?? 0) > 0) {
      keyOpts = { toneHz: Number(mp.ctcss_hz) };
    } else if (isHam && Number(mp.dcs_code ?? 0) > 0) {
      keyOpts = { dcsCode: Number(mp.dcs_code), dcsInvert: Number(mp.dcs_invert ?? 0) !== 0 };
    }
    const r = await state.client.keyTx(keyOpts);
    const via = r.offset_hz
      ? ` via repeater (${offsetLabel(r.offset_hz)}${r.tone_hz ? `, ${r.tone_hz.toFixed(1)} Hz` : ""})`
      : r.tone_hz
        ? ` · CTCSS ${Number(r.tone_hz).toFixed(1)} Hz`
        : r.dcs_code
          ? ` · DCS D${String(r.dcs_code).padStart(3, "0")}${r.dcs_invert ? "I" : "N"}`
          : "";
    logLine(
      micStream
        ? `PTT keyed — mic live, ${r.gain_db} dB${via}`
        : `PTT keyed — no mic granted, transmitting silence (${r.gain_db} dB)${via}`,
    );
    pttTimer = window.setTimeout(() => {
      logLine("PTT auto-released (max hold time)");
      void pttUp();
    }, PTT_MAX_HOLD_MS);
  } catch (e) {
    micStream?.getAudioTracks().forEach((t) => (t.enabled = false));
    setState({ pttHeld: false });
    logLine(`PTT key failed — ${(e as ApiError).message}`);
  }
}

async function refreshTxLog(): Promise<void> {
  if (!state.client) return;
  try {
    setState({ txLog: (await state.client.txLog()).slice(0, 8) });
  } catch {
    /* audit log is best-effort UI sugar */
  }
}

async function pttUp(): Promise<void> {
  if (pttTimer) {
    window.clearTimeout(pttTimer);
    pttTimer = undefined;
  }
  if (!state.pttHeld) return;
  setState({ pttHeld: false });
  micStream?.getAudioTracks().forEach((t) => (t.enabled = false));
  if (!state.client) return;
  try {
    await state.client.unkeyTx();
    logLine("PTT released");
  } catch (e) {
    logLine(`PTT unkey failed — ${(e as ApiError).message}`);
  }
  void refreshTxLog();
}

// --- station finders --------------------------------------------------

function setLocation(loc: Located, note?: string): void {
  localStorage.setItem(LOC_KEY, `${loc.lat}, ${loc.lon}`);
  setState({
    loc,
    locNote: note ?? `location ${loc.lat.toFixed(3)}, ${loc.lon.toFixed(3)}`,
  });
  void refreshStations();
}

function findFromInput(): void {
  const text = panels.querySelector<HTMLInputElement>("#loc")?.value ?? "";
  const parsed = parseLatLon(text);
  if (!parsed) {
    setState({ locNote: "enter your location as `lat, lon` (e.g. 40.76, -111.89)" });
    return;
  }
  setLocation(parsed);
}

function useMyLocation(): void {
  if (!navigator.geolocation) {
    setState({ locNote: "geolocation unavailable — type lat, lon" });
    return;
  }
  setState({ locNote: "locating…" });
  navigator.geolocation.getCurrentPosition(
    (pos) => setLocation({ lat: +pos.coords.latitude.toFixed(5), lon: +pos.coords.longitude.toFixed(5) }),
    (err) => setState({ locNote: `location failed (${err.message}) — type lat, lon` }),
    { enableHighAccuracy: false, timeout: 10_000, maximumAge: 600_000 },
  );
}

let repeaterBook: Repeater[] | null = null;

/** Rebuild `state.repeaters` for the current ham band: bundled + manual,
 *  distance-sorted when a location is set (manual rows without coords sink to
 *  the bottom). Loads the bundled list once, then filters from memory. */
async function rebuildRepeaterList(bandId?: string): Promise<void> {
  if (state.radio?.mode !== "ham") return;
  if (!repeaterBook) {
    try {
      repeaterBook = await loadRepeaters();
    } catch (e) {
      repeaterBook = [];
      setState({ locNote: `repeater list failed to load (${(e as Error).message})` });
    }
  }
  const band = bandId ?? hamBandContaining(state.radio.frequency_hz)?.id ?? state.hamBand;
  const all = [...state.manualRepeaters, ...repeaterBook].filter((r) => r.band === band);
  const loc = state.loc;
  const withDist: NearRepeater[] = all.map((r) => ({
    ...r,
    distance_mi:
      loc && r.lat && r.lon ? Math.round(haversineMi(loc.lat, loc.lon, r.lat, r.lon)) : null,
  }));
  withDist.sort((a, b) => {
    if (a.distance_mi == null && b.distance_mi == null) return a.output_hz - b.output_hz;
    if (a.distance_mi == null) return 1;
    if (b.distance_mi == null) return -1;
    return a.distance_mi - b.distance_mi;
  });
  // `repeaters.json` is nationwide (~9k), so show the nearest slice — the
  // operator's own manual rows are always kept regardless of distance.
  const LIMIT = 60;
  const shown = withDist.slice(0, LIMIT);
  for (const r of withDist) {
    if (r.manual && !shown.includes(r)) shown.push(r);
  }
  setState({ repeaters: shown });
}

async function refreshStations(): Promise<void> {
  const mode = state.radio?.mode;
  if (mode === "ham") void rebuildRepeaterList();
  const loc = state.loc;
  if (!loc) return;
  try {
    if (mode === "nbfm") {
      const list = await loadNwrStations();
      setState({ nwrNearby: nearestNwr(loc.lat, loc.lon, list, 20) });
    } else if (mode === "wbfm") {
      const list = await loadFmStations();
      setState({ fmNearby: nearestFm(loc.lat, loc.lon, list, 24) });
    } else if (mode === "am") {
      const list = await loadAmStations();
      setState({ amNearby: nearestAm(loc.lat, loc.lon, list, 24) });
    } else if (mode === "apt") {
      const tles = await loadAptTle();
      const aptPasses: Record<string, AptPass[]> = {};
      for (const tle of tles) aptPasses[tle.name] = nextPasses(tle, loc.lat, loc.lon);
      setState({ aptPasses });
    }
  } catch (e) {
    setState({ locNote: `station list failed to load (${(e as Error).message})` });
  }
}

/** Identify the station at the current frequency, if the list has a match. */
function tunedStationLabel(): string | null {
  const r = state.radio;
  if (!r) return null;
  if (r.mode === "nbfm") {
    const s = state.nwrNearby.find((x) => x.freq_hz === r.frequency_hz);
    return s ? `${s.callsign} · ${s.site}, ${s.state}` : null;
  }
  if (r.mode === "wbfm") {
    const s = state.fmNearby.find((x) => x.freq_hz === r.frequency_hz);
    return s ? `${s.call} · ${s.city}, ${s.state}` : null;
  }
  if (r.mode === "am") {
    const s = state.amNearby.find((x) => x.freq_hz === r.frequency_hz);
    return s ? `${s.call} · ${s.city}, ${s.state}` : null;
  }
  if (r.mode === "apt") {
    const s = APT_SATELLITES.find((x) => x.freq_hz === r.frequency_hz);
    return s ? s.name : null;
  }
  if (r.mode === "frs") {
    const c = frsChannelAt(r.frequency_hz);
    return c ? `${c.shared_gmrs ? "FRS/GMRS shared" : "FRS only"} · ${c.max_power_w} W max` : null;
  }
  return null;
}

// --- timers / poll --------------------------------------------------

function startTimers(): void {
  stopTimers();
  const hbMs = Math.max(3, state.session?.heartbeat_interval_s ?? 15) * 1000;
  heartbeatTimer = window.setInterval(tickHeartbeat, hbMs);
  pollTimer = window.setInterval(poll, 1000);
}

function stopTimers(): void {
  if (heartbeatTimer) window.clearInterval(heartbeatTimer);
  if (pollTimer) window.clearInterval(pollTimer);
  heartbeatTimer = pollTimer = undefined;
  pollFails = 0;
}

async function tickHeartbeat(): Promise<void> {
  if (!state.client || !state.session) return;
  try {
    await state.client.heartbeat(state.session.session_id);
  } catch (e) {
    loopFailed(e as ApiError, "heartbeat");
  }
}

async function poll(): Promise<void> {
  if (!state.client) return;
  try {
    const [server, radio, status] = await Promise.all([
      state.client.server(),
      state.client.radio(),
      state.client.radioStatus(),
    ]);
    pollFails = 0;
    const dataMode =
      radio.mode === "adsb" ||
      radio.mode === "ais" ||
      radio.mode === "aprs" ||
      radio.mode === "apt";
    let audioStats = state.audioStats;
    if (!dataMode && state.session && state.audio && state.audioState !== "idle") {
      audioStats = await state.client
        .audioState(state.session.session_id)
        .catch(() => state.audioStats);
    }
    let adsb = state.adsb;
    let ais = state.ais;
    let aprs = state.aprs;
    let aprsPackets = state.aprsPackets;
    let apt = state.apt;
    if (radio.mode === "adsb") {
      adsb = await state.client.adsbAircraft().catch(() => state.adsb);
    } else if (radio.mode === "ais") {
      ais = await state.client.aisVessels().catch(() => state.ais);
    } else if (radio.mode === "aprs") {
      aprs = await state.client.aprsStations().catch(() => state.aprs);
      aprsPackets = await state.client
        .aprsPackets()
        .then((r) => r.packets)
        .catch(() => state.aprsPackets);
    } else if (radio.mode === "apt") {
      apt = await state.client.aptStatus().catch(() => state.apt);
      // The raster can grow to a couple MB; refetch it far less often than
      // the 1s status/telemetry cadence.
      if (Date.now() - aptImageFetchedAt > 3_000) {
        aptImageFetchedAt = Date.now();
        void refreshAptImage();
      }
    }
    let transcript = state.transcript;
    if (
      server.capabilities.includes("stt") &&
      ["frs", "ham", "nbfm", "wbfm", "am"].includes(radio.mode)
    ) {
      transcript = await state.client
        .transcript()
        .then((r) => r.segments)
        .catch(() => state.transcript);
    }
    setState({ server, radio, status, audioStats, adsb, ais, aprs, aprsPackets, transcript, apt });
  } catch (e) {
    pollFails += 1;
    if (pollFails >= 3) loopFailed(e as ApiError, "poll");
  }
}

function loopFailed(err: ApiError, where: string): void {
  stopTimers();
  void teardownAudio();
  state.client = null;
  state.session = null;
  setState({ phase: "error", error: `lost connection (${where}: ${err.message})` });
  logLine(`connection lost — ${where}: ${err.message}`);
}

// --- state + render ----------------------------------------------------

function setState(patch: Partial<State>): void {
  Object.assign(state, patch);
  render();
}

function logLine(msg: string): void {
  const ts = new Date().toLocaleTimeString();
  state.log.unshift(`${ts}  ${msg}`);
  state.log = state.log.slice(0, 40);
  render();
}

// --- render (structure vs. live values) ------------------------------

let builtKey = "";

function structKey(): string {
  return [
    state.phase,
    state.radio?.mode ?? "",
    state.bandTab,
    state.radio?.mode === "ham"
      ? `${hamBandContaining(state.radio.frequency_hz)?.id ?? state.hamBand}:${
          Number(state.radio.mode_params.ctcss_hz ?? 0) > 0
        }:${Number(state.radio.mode_params.dcs_code ?? 0) > 0}:${state.hamView}:${state.rpAddOpen}:${state.repeaters.length}`
      : "",
    state.switching,
    state.seeking,
    state.modes.length,
    state.nwrNearby.length,
    state.fmNearby.length,
    state.amNearby.length,
    state.aptPasses ? 1 : 0,
    state.loc ? 1 : 0,
    state.locNote ?? "",
    state.audioStats ? 1 : 0,
    state.adsb || state.ais || state.aprs ? 1 : 0,
    state.apt ? 1 : 0,
    state.scopeRangeNm,
    state.device?.id ?? state.device?.label ?? "",
    state.devices.length,
  ].join("|");
}

function render(): void {
  const connected = state.phase === "connected";
  const connecting = state.phase === "connecting";

  // Fleet member? Show its label in the header and a link back to the
  // hypervisor's radio picker.
  const fleetUrl = connected ? state.server?.fleet_url ?? null : null;
  const label = connected ? state.server?.instance_label ?? null : null;
  appSub.textContent = label ? `radio · ${label}` : "SDR on your LAN";
  fleetLink.hidden = !fleetUrl;
  if (fleetUrl) fleetLink.href = fleetUrl;

  actionBtn.textContent = connected
    ? "Disconnect"
    : connecting
      ? "Cancel connecting…"
      : "Connect";
  // Stay clickable while connecting — a wedged request (fetch has no bound on
  // how long it can hang in a backgrounded WebView) would otherwise leave no
  // way out short of restarting the app.
  actionBtn.disabled = false;
  actionBtn.classList.toggle("secondary", connected || connecting);
  hostInput.disabled = connected || connecting;
  connStatus.innerHTML = connStatusHtml();

  if (!connected) {
    panels.innerHTML = idlePanelsHtml();
    builtKey = "";
    return;
  }

  const key = structKey();
  if (key !== builtKey) {
    panels.innerHTML = panelsHtml();
    builtKey = key;
  }
  patchLive();
}

const q = <T extends Element>(sel: string) => panels.querySelector<T>(sel);
const setHTML = (sel: string, html: string) => {
  const el = q(sel);
  if (el && el.innerHTML !== html) el.innerHTML = html;
};


/** Mount / rehost / tear down the Receiver Analysis panel to match the
 *  current mode. The `AnalysisView` keeps its offscreen waterfall + view
 *  state across a `panels` rebuild; only its visible DOM is remounted. */
function syncAnalysisView(): void {
  const wantHost =
    state.radio?.mode === "analysis" ? panels.querySelector<HTMLElement>("#analysis-host") : null;
  if (!wantHost) {
    if (analysisView) {
      analysisView.destroy();
      analysisView = null;
    }
    return;
  }
  if (!state.client) return;
  if (!analysisView) {
    analysisView = new AnalysisView(
      wantHost,
      state.client,
      (hz) => void tuneFrequency(hz),
      state.radio?.frequency_hz ?? 0,
    );
    analysisView.start();
  } else if (analysisView.host !== wantHost) {
    analysisView.rehost(wantHost);
  }
}

/** Update the values that change every poll without touching the DOM
 *  structure (so the station list keeps its scroll position). */
function patchLive(): void {
  if (!state.radio) return;
  setHTML("#now-playing", nowPlayingInner());
  setHTML("#audio-card", audioInner());
  setHTML("#telemetry", telemetryInner());
  setHTML("#log", state.log.map(esc).join("\n") || "—");

  if (state.radio.mode === "adsb" || state.radio.mode === "ais" || state.radio.mode === "aprs") {
    setHTML("#scope-list", scopeListInner());
    drawScope();
    if (state.radio.mode === "aprs") {
      const fq = q<HTMLInputElement>("#aprs-freq");
      if (fq && fq !== document.activeElement) {
        const v = (state.radio.frequency_hz / 1e6).toFixed(4);
        if (fq.value !== v) fq.value = v;
      }
      const log = q<HTMLElement>("#aprs-log");
      if (log) {
        const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
        const html = aprsLogInner();
        if (log.innerHTML !== html) {
          log.innerHTML = html;
          if (atBottom) log.scrollTop = log.scrollHeight;
        }
      }
    }
  } else if (state.radio.mode === "apt") {
    setHTML("#apt-status", aptStatusInner());
    setHTML("#apt-sat-rows", aptSatRowsInner());
    drawAptImage();
  }

  const freq = state.radio.frequency_hz;
  panels.querySelectorAll<HTMLButtonElement>(".station").forEach((b) => {
    b.classList.toggle("tuned", Number(b.dataset.hz) === freq);
  });

  // Voice-text card (frs/ham): live transcript + TX-keyed lockout.
  const vtLog = q<HTMLElement>("#vt-log");
  if (vtLog) {
    const atBottom = vtLog.scrollHeight - vtLog.scrollTop - vtLog.clientHeight < 40;
    const html = transcriptInner();
    if (vtLog.innerHTML !== html) {
      vtLog.innerHTML = html;
      if (atBottom) vtLog.scrollTop = vtLog.scrollHeight;
    }
  }
  const vtBusy = q<HTMLElement>("#vt-busy");
  if (vtBusy) vtBusy.hidden = !(state.status?.dsp.transcribing ?? false);
  const keyed = state.status?.dsp.tx_keyed ?? false;
  const vtMsg = q<HTMLInputElement>("#vt-msg");
  const vtSay = q<HTMLButtonElement>("#vt-say");
  if (vtMsg && vtMsg !== document.activeElement) vtMsg.disabled = keyed;
  if (vtSay) {
    vtSay.disabled = keyed;
    vtSay.textContent = keyed ? "on air…" : "Speak";
  }

  // Keep the manual-tune dial's number in sync as +/- and Seek move the
  // frequency — structKey() ignores frequency_hz, so the panel isn't
  // rebuilt. Don't stomp a value the user is mid-edit on.
  syncAnalysisView();

  const dial = q<HTMLInputElement>("#fm-freq, #am-freq, #apt-freq, #ham-freq");
  if (dial && dial !== document.activeElement) {
    const v =
      dial.id === "am-freq"
        ? (freq / 1e3).toFixed(0)
        : dial.id === "apt-freq" || dial.id === "ham-freq"
          ? (freq / 1e6).toFixed(4)
          : (freq / 1e6).toFixed(1);
    if (dial.value !== v) dial.value = v;
  }

  // Band-plan reference: move the "you are here" highlight as +/- steps the
  // dial (structKey only rebuilds the panel when the *band* changes).
  if (state.radio.mode === "ham") {
    panels.querySelectorAll<HTMLElement>(".ham-seg").forEach((el) => {
      const [lo, hi] = (el.dataset.seg ?? "0 0").split(" ").map(Number);
      el.classList.toggle("here", freq >= lo! && freq < hi!);
    });
    const det = q("#ham-ctcss-det");
    if (det) {
      const tone = state.status?.dsp.ctcss_tone_hz;
      const dcs = state.status?.dsp.dcs_code;
      const cfgDcs = Number(state.radio.mode_params.dcs_code ?? 0);
      const on = cfgDcs > 0 ? dcs != null : tone != null;
      det.textContent = on
        ? cfgDcs > 0
          ? `decoded D${String(dcs).padStart(3, "0")} ✓`
          : `detected ${tone!.toFixed(1)} Hz ✓`
        : "not detected";
      det.classList.toggle("ok", on);
    }
    const scanEl = q("#ham-scan");
    const useBtn = q<HTMLButtonElement>("#ham-ctcss-use");
    const scan = state.status?.dsp.ctcss_scan_hz ?? null;
    const cfg = Number(state.radio.mode_params.ctcss_hz ?? 0);
    if (scanEl) {
      scanEl.textContent = scan != null ? `${scan.toFixed(1)} Hz` : "no tone";
      scanEl.classList.toggle("ok", scan != null);
    }
    if (useBtn) {
      const offer = scan != null && Math.abs(scan - cfg) > 0.05;
      useBtn.hidden = !offer;
      if (offer) useBtn.dataset.hz = String(scan);
    }
  }
}

// --- delegated events (wired once) ----------------------------------

function installDelegates(): void {
  panels.addEventListener("change", (e) => {
    const t = e.target as HTMLElement;
    if (t.id === "device-pick") void switchDevice((t as HTMLSelectElement).value);
  });

  panels.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const hit = (sel: string) => t.closest(sel);

    const mode = t.closest<HTMLButtonElement>(".mode-btn");
    if (mode) return void switchMode(mode.dataset.mode!);
    if (hit("#radio-toggle")) return void toggleRadio();
    if (hit("#radio-options")) return void openRadioOptionsPanel();
    if (hit("#aprs-send")) return void sendAprsMsg();
    if (hit("#aprs-beacon")) return void beaconAprs();
    if (hit("#aprs-freq-go")) return void aprsTune();
    if (hit("#vt-say")) return void sendVoiceText();
    if (hit("#audio-toggle")) return void toggleAudio();
    if (hit("#audio-rec")) return void toggleAudioRecord();
    if (hit("#loc-find")) return findFromInput();
    if (hit("#loc-me")) return useMyLocation();
    if (hit("#seek-down")) return void seek(-1);
    if (hit("#seek-up")) return void seek(1);
    if (hit("#scan-toggle")) return void toggleScan();
    if (hit("#scope-recenter")) return setState({ scopeSel: null });
    if (t.id === "scope") return onScopeClick(e as MouseEvent);

    const acRow = t.closest<HTMLButtonElement>(".ac-row");
    if (acRow) {
      const cid = acRow.dataset.cid ?? null;
      const next = cid === state.scopeSel ? null : cid;
      setState({ scopeSel: next });
      if (next) enrichSelected(next);
      return;
    }
    if (hit("#fm-down")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, -1));
    if (hit("#fm-up")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, 1));
    if (hit("#fm-go")) return fmManualGo();
    if (hit("#am-down")) return void tuneFrequency(stepAm(state.radio!.frequency_hz, -1));
    if (hit("#am-up")) return void tuneFrequency(stepAm(state.radio!.frequency_hz, 1));
    if (hit("#am-go")) return amManualGo();
    if (hit("#apt-go")) return aptManualGo();
    if (hit("#ham-down")) return void tuneFrequency(hamStep(state.radio!.frequency_hz, -1));
    if (hit("#ham-up")) return void tuneFrequency(hamStep(state.radio!.frequency_hz, 1));
    if (hit("#ham-go")) return hamManualGo();
    if (hit("#rp-add-toggle")) return setState({ rpAddOpen: true });
    if (hit("#rp-add-cancel")) return setState({ rpAddOpen: false });
    if (hit("#rp-add-save")) return saveManualRepeater();
    if (hit("#rp-paste-fill")) return fillRepeaterFromShorthand();
    if (hit("#ham-ctcss-use")) {
      const sel = panels.querySelector<HTMLSelectElement>("#ham-ctcss");
      const hz = (t.closest<HTMLElement>("#ham-ctcss-use")?.dataset.hz ?? "").trim();
      if (sel && hz) {
        sel.value = hz;
        void applyHamSquelch("ctcss");
      }
      return;
    }

    const rpDel = t.closest<HTMLElement>("[data-rpdel]");
    if (rpDel) {
      e.stopPropagation();
      return deleteManualRepeater(rpDel.dataset.rpdel!);
    }
    const rpRow = t.closest<HTMLButtonElement>(".repeater");
    if (rpRow) {
      const rp = state.repeaters.find((x) => x.id === rpRow.dataset.rpid);
      if (rp) void tuneRepeater(rp);
      return;
    }

    const hamView = t.closest<HTMLButtonElement>("[data-hamview]");
    if (hamView) {
      const v = hamView.dataset.hamview === "repeater" ? "repeater" : "simplex";
      localStorage.setItem("lanline.hamView", v);
      setState({ hamView: v });
      if (v === "repeater") void rebuildRepeaterList();
      return;
    }

    const hamTab = t.closest<HTMLButtonElement>("[data-hamband]");
    if (hamTab) {
      const b = hamBand(hamTab.dataset.hamband!);
      localStorage.setItem("lanline.hamBand", b.id);
      setState({ hamBand: b.id });
      if (state.hamView === "repeater") void rebuildRepeaterList(b.id);
      return void tuneFrequency(b.defaultHz);
    }

    const tab = t.closest<HTMLButtonElement>("[data-bandtab]");
    if (tab) return setState({ bandTab: tab.dataset.bandtab as "nearby" | "manual" });

    const station = t.closest<HTMLButtonElement>(".station");
    if (station) {
      const hz = Number(station.dataset.hz);
      if (hz) void tuneFrequency(hz, station.dataset.label);
    }
  });

  panels.addEventListener("change", (e) => {
    const el = e.target as HTMLElement;
    if (el.id === "audio-mute") toggleMute();
    if (el.id === "ham-ctcss") void applyHamSquelch("ctcss");
    if (el.id === "ham-dcs") void applyHamSquelch("dcs");
    if (el.id === "ham-dcs-inv" || el.id === "ham-ctcss-mon") void applyHamSquelch();
    if (el.id === "aprs-mycall") {
      setAprsCall("my", (el as HTMLInputElement).value);
      const log = q<HTMLElement>("#aprs-log");
      if (log) log.innerHTML = aprsLogInner(); // re-classify own vs. others
    }
    if (el.id === "aprs-tocall") setAprsCall("to", (el as HTMLInputElement).value);
    if (el.id === "scope-range") {
      const v = (el as HTMLSelectElement).value;
      setState({ scopeRangeNm: v === "auto" ? "auto" : Number(v) });
    }
  });

  panels.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key !== "Enter") return;
    const id = (e.target as HTMLElement).id;
    if (id === "loc") findFromInput();
    else if (id === "fm-freq") fmManualGo();
    else if (id === "am-freq") amManualGo();
    else if (id === "apt-freq") aptManualGo();
    else if (id === "ham-freq") hamManualGo();
    else if (id === "rp-paste") fillRepeaterFromShorthand();
    else if (id === "aprs-msg") void sendAprsMsg();
    else if (id === "aprs-freq") aprsTune();
    else if (id === "vt-msg") void sendVoiceText();
  });

  // Press-and-hold, not click: pointerdown keys, pointerup/cancel unkeys.
  // The release listeners are on `document` (not `panels`) so a drag off
  // the button — or off the app entirely — still reliably releases PTT;
  // the alternative (button-scoped only) can strand a "still transmitting"
  // state if the pointer leaves the element before lifting.
  panels.addEventListener("pointerdown", (e) => {
    if ((e.target as HTMLElement).closest("#ptt-button")) {
      e.preventDefault();
      void pttDown();
    }
  });
  document.addEventListener("pointerup", () => {
    if (state.pttHeld) void pttUp();
  });
  document.addEventListener("pointercancel", () => {
    if (state.pttHeld) void pttUp();
  });

  // Spacebar mirrors the "Hold to talk" button — hold to key, release to
  // stop. Ignored while typing in a field, while the Radio options modal is
  // open, and unless PTT is actually available (frs / ham).
  const pttHotkeyOk = (t: EventTarget | null): boolean => {
    const m = state.radio?.mode;
    if (
      (m !== "frs" && m !== "ham") ||
      !state.radio?.running ||
      !state.server?.capabilities.includes("ptt") ||
      radioOptionsOpen()
    ) {
      return false;
    }
    const el = t as HTMLElement | null;
    return !el || !(el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
  };
  document.addEventListener("keydown", (e) => {
    if (e.code !== "Space" || e.repeat || !pttHotkeyOk(e.target)) return;
    e.preventDefault();
    void pttDown();
  });
  document.addEventListener("keyup", (e) => {
    if (e.code !== "Space" || !state.pttHeld) return;
    e.preventDefault();
    void pttUp();
  });
  // Held space + tab-away would otherwise strand a keyed transmission.
  window.addEventListener("blur", () => {
    if (state.pttHeld) void pttUp();
  });
}

function fmManualGo(): void {
  const raw = panels.querySelector<HTMLInputElement>("#fm-freq")?.value ?? "";
  const mhz = parseFloat(raw);
  if (Number.isFinite(mhz)) void tuneFrequency(snapFm(mhz * 1e6));
}

function amManualGo(): void {
  const raw = panels.querySelector<HTMLInputElement>("#am-freq")?.value ?? "";
  const khz = parseFloat(raw);
  if (Number.isFinite(khz)) void tuneFrequency(snapAm(khz * 1e3));
}

function aptManualGo(): void {
  const raw = panels.querySelector<HTMLInputElement>("#apt-freq")?.value ?? "";
  const mhz = parseFloat(raw);
  if (Number.isFinite(mhz)) {
    const hz = Math.min(APT_MAX_HZ, Math.max(APT_MIN_HZ, Math.round(mhz * 1e6)));
    void tuneFrequency(hz);
  }
}

// --- panel HTML ----------------------------------------------------

function idlePanelsHtml(): string {
  return `
    <section class="card">
      <h2>Not connected</h2>
      <p class="note">Opening this page from the server's own address
      (<code>http://lanline.local:8730/</code>) connects automatically. Otherwise
      enter the host shown in the server's startup log. The Android app discovers
      servers over the LAN on its own.</p>
    </section>
    <section class="card"><h2>Event log</h2><div id="log">${state.log.map(esc).join("\n") || "—"}</div></section>
  `;
}

function panelsHtml(): string {
  const dataMode =
    state.radio?.mode === "adsb" ||
    state.radio?.mode === "ais" ||
    state.radio?.mode === "apt" ||
    state.radio?.mode === "analysis";
  return `
    ${modeStripHtml()}
    ${wizardHtml()}
    <section class="card" id="now-playing">${nowPlayingInner()}</section>
    ${dataMode ? "" : `<section class="card" id="audio-card">${audioInner()}</section>`}
    <section class="card" id="telemetry">${telemetryInner()}</section>
    ${serverHtml()}
    <section class="card"><h2>Event log</h2><div id="log">${state.log.map(esc).join("\n") || "—"}</div></section>
  `;
}

function modeStripHtml(): string {
  const cur = state.radio?.mode;
  const ids = state.modes.length ? state.modes.map((m) => m.id) : Object.keys(MODE_META);
  return `
    <section class="card">
      <h2>Mode</h2>
      <div class="mode-strip">
        ${ids
          .map((id) => {
            const meta = MODE_META[id] ?? { label: id, icon: "•", band: "" };
            const outOfRange = !modeInDeviceRange(id);
            return `<button class="mode-btn${id === cur ? " on" : ""}${outOfRange ? " oor" : ""}" data-mode="${esc(id)}" ${state.switching || outOfRange ? "disabled" : ""} ${outOfRange ? `title="outside ${esc(state.device?.label ?? "this device")}'s tuning range"` : ""}>
              <span class="m-icon">${meta.icon}</span>
              <span class="m-label">${esc(meta.label)}</span>
              <span class="m-band">${esc(meta.band)}</span>
            </button>`;
          })
          .join("")}
      </div>
      ${state.switching ? `<p class="note" style="margin:10px 0 0">switching…</p>` : ""}
    </section>`;
}

function locRowHtml(): string {
  const saved = localStorage.getItem(LOC_KEY) ?? "";
  return `
    <div class="connect-row">
      <input id="loc" type="text" spellcheck="false" autocapitalize="off"
             placeholder="your location — lat, lon" value="${esc(saved)}" />
      <button id="loc-me" class="secondary">📍 My location</button>
      <button id="loc-find">Find</button>
    </div>
    ${state.locNote ? `<p class="note" style="margin:8px 0 0">${esc(state.locNote)}</p>` : ""}`;
}

function stationRow(hz: number, freqLabel: string, name: string, meta: string, tuned: boolean, label: string): string {
  return `<button class="station${tuned ? " tuned" : ""}" data-hz="${hz}" data-label="${esc(label)}">
    <span class="s-call">${esc(freqLabel)}</span>
    <span class="s-site">${esc(name)}</span>
    <span class="s-meta">${esc(meta)}</span>
  </button>`;
}

// --- APRS messaging (chat between two radios) ----------------------

function aprsCallKey(which: "my" | "to"): string {
  return `lanline.aprs.${which}call.${state.base}`;
}
function aprsCall(which: "my" | "to"): string {
  try {
    return localStorage.getItem(aprsCallKey(which)) ?? "";
  } catch {
    return "";
  }
}
function setAprsCall(which: "my" | "to", v: string): void {
  try {
    localStorage.setItem(aprsCallKey(which), v.trim().toUpperCase());
  } catch {
    /* private mode */
  }
}

function aprsLogInner(): string {
  const msgs = parseAprsMessages(state.aprsPackets);
  if (!msgs.length) return `<div class="note" style="padding:8px 4px">No messages yet.</div>`;
  const mine = aprsCall("my").toUpperCase();
  return msgs
    .slice(-50)
    .map((m) => {
      const out = mine !== "" && m.from.toUpperCase() === mine;
      return `<div class="aprs-msg ${out ? "out" : "in"}">
        <span class="aprs-msg-who">${esc(out ? `→ ${m.to}` : m.from)}</span>
        <span class="aprs-msg-text">${esc(m.text)}</span>
      </div>`;
    })
    .join("");
}

function aprsChatHtml(): string {
  const mp = state.radio?.mode_params ?? {};
  const hasRef = Number(mp.reference_lat ?? 0) !== 0 || Number(mp.reference_lon ?? 0) !== 0;
  const tx = state.server?.capabilities.includes("ptt") ?? false;
  const mhz = ((state.radio?.frequency_hz ?? 144_390_000) / 1e6).toFixed(4);
  return `
    <section class="card" id="aprs-chat">
      <h2>APRS messaging</h2>
      ${
        tx
          ? ""
          : `<p class="note warn" style="margin:0 0 10px">transmit is disabled on this server (—enable-tx) — receive only</p>`
      }
      <div class="aprs-freq">
        <input id="aprs-freq" type="text" inputmode="decimal" spellcheck="false" value="${mhz}" />
        <span class="dial-unit">MHz</span>
        <button id="aprs-freq-go" class="secondary">Tune</button>
        <span class="note" style="margin-left:auto">144.390 = 2 m APRS</span>
      </div>
      <div class="aprs-calls">
        <input id="aprs-mycall" type="text" spellcheck="false" autocapitalize="characters"
          placeholder="your call e.g. KZ4AZ-1" value="${esc(aprsCall("my"))}" />
        <span class="dial-unit">→</span>
        <input id="aprs-tocall" type="text" spellcheck="false" autocapitalize="characters"
          placeholder="their call" value="${esc(aprsCall("to"))}" />
      </div>
      <div id="aprs-log" class="stations aprs-log">${aprsLogInner()}</div>
      <div class="aprs-send-row">
        <input id="aprs-msg" type="text" maxlength="67" placeholder="message…" ${tx ? "" : "disabled"} />
        <button id="aprs-send" ${tx ? "" : "disabled"}>Send</button>
        <button id="aprs-beacon" class="secondary" ${tx && hasRef ? "" : "disabled"}
          title="${hasRef ? "beacon your reference position" : "set a reference position first"}">Beacon</button>
      </div>
    </section>`;
}

function aprsTune(): void {
  const v = Number((q<HTMLInputElement>("#aprs-freq")?.value ?? "").trim());
  if (!Number.isFinite(v) || v <= 0) return;
  void tuneFrequency(Math.round(v * 1e6));
}

async function sendAprsMsg(): Promise<void> {
  if (!state.client) return;
  const my = (q<HTMLInputElement>("#aprs-mycall")?.value ?? "").trim().toUpperCase();
  const to = (q<HTMLInputElement>("#aprs-tocall")?.value ?? "").trim().toUpperCase();
  const inp = q<HTMLInputElement>("#aprs-msg");
  const text = (inp?.value ?? "").trim();
  if (!my) return void logLine("APRS: set your callsign first");
  if (!to) return void logLine("APRS: set the destination callsign");
  if (!text) return;
  try {
    const r = await state.client.aprsTx({ source: my, message_to: to, message_text: text });
    logLine(`APRS TX: ${r.tnc2}`);
    if (inp) inp.value = "";
  } catch (e) {
    logLine(`APRS TX failed: ${e instanceof ApiError ? e.message : String(e)}`);
  }
}

async function beaconAprs(): Promise<void> {
  if (!state.client || !state.radio) return;
  const my = (q<HTMLInputElement>("#aprs-mycall")?.value ?? "").trim().toUpperCase();
  if (!my) return void logLine("APRS: set your callsign first");
  const mp = state.radio.mode_params;
  const lat = Number(mp.reference_lat ?? 0);
  const lon = Number(mp.reference_lon ?? 0);
  if (lat === 0 && lon === 0) return void logLine("APRS: no reference position set");
  try {
    const r = await state.client.aprsTx({ source: my, lat, lon, comment: "LANline" });
    logLine(`APRS beacon: ${r.tnc2}`);
  } catch (e) {
    logLine(`APRS beacon failed: ${e instanceof ApiError ? e.message : String(e)}`);
  }
}

// --- Voice ↔ text (TTS on TX, STT on RX) --------------------------

function transcriptInner(): string {
  if (!state.transcript.length) {
    return `<div class="note" style="padding:8px 4px">No received transmissions yet.</div>`;
  }
  return state.transcript
    .slice(-40)
    .map((s) => {
      const t = new Date(s.time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      return `<div class="aprs-msg in">
        <span class="aprs-msg-who">${t} · ${s.rssi_dbfs.toFixed(0)} dBFS · ${s.secs.toFixed(1)}s</span>
        <span class="aprs-msg-text">${esc(s.text)}</span>
      </div>`;
    })
    .join("");
}

function voiceTextCard(): string {
  const caps = state.server?.capabilities ?? [];
  const tts = caps.includes("tts");
  const stt = caps.includes("stt");
  if (!tts && !stt) return "";
  const keyed = state.status?.dsp.tx_keyed ?? false;
  return `
    <section class="card" id="voice-text">
      <h2>Voice text</h2>
      ${
        tts
          ? `<div class="vt-send">
        <input id="vt-msg" type="text" maxlength="500"
          placeholder="type a message to speak on the air…" ${keyed ? "disabled" : ""} />
        <button id="vt-say" ${keyed ? "disabled" : ""}>${keyed ? "on air…" : "Speak"}</button>
      </div>`
          : `<p class="note warn" style="margin:0">text-to-speech is not enabled (—tts) — receive only</p>`
      }
      ${
        stt
          ? `<div id="vt-log" class="stations vt-log">${transcriptInner()}</div>
             <p class="note" id="vt-busy" style="margin:6px 0 0" hidden>transcribing…</p>`
          : ""
      }
    </section>`;
}

async function sendVoiceText(): Promise<void> {
  if (!state.client) return;
  const inp = q<HTMLInputElement>("#vt-msg");
  const text = (inp?.value ?? "").trim();
  if (!text) return;
  const btn = q<HTMLButtonElement>("#vt-say");
  if (btn) btn.disabled = true;
  try {
    const r = await state.client.saySpeech({ text });
    logLine(`spoke (${r.backend}, ${r.secs.toFixed(1)}s): "${r.text}"`);
    if (inp) inp.value = "";
  } catch (e) {
    logLine(`speak failed: ${e instanceof ApiError ? e.message : String(e)}`);
  } finally {
    if (btn) btn.disabled = false;
  }
}

function wizardHtml(): string {
  switch (state.radio?.mode) {
    case "nbfm":
      return nwrWizardHtml();
    case "wbfm":
      return fmWizardHtml();
    case "am":
      return amWizardHtml();
    case "apt":
      return aptWizardHtml();
    case "frs":
      return frsWizardHtml() + voiceTextCard();
    case "ham":
      return hamWizardHtml() + voiceTextCard();
    case "adsb":
    case "ais":
      return scopeWizardHtml();
    case "aprs":
      return scopeWizardHtml() + aprsChatHtml();
    case "analysis":
      return `<section class="card"><h2>Receiver Analysis</h2><div id="analysis-host"></div></section>`;
    case "debug_tone":
      return toneWizardHtml();
    default:
      return "";
  }
}

function nwrWizardHtml(): string {
  const r = state.radio!;
  // NWR's 7 channels are shared: several nearby transmitters can sit on the
  // tuned frequency at once. Highlight only the closest one (the list is
  // distance-sorted) so the picker shows a single active row, matching what
  // the radio actually receives (the strongest signal on that channel).
  const tunedIdx = state.nwrNearby.findIndex((s) => s.freq_hz === r.frequency_hz);
  const rows = state.nwrNearby
    .map((s, i) => {
      const flag =
        s.status === "out_of_service"
          ? "out of service"
          : s.status === "degraded"
            ? "degraded"
            : `${s.distance_mi.toFixed(0)} mi`;
      return stationRow(
        s.freq_hz,
        (s.freq_hz / 1e6).toFixed(3),
        `${s.site}, ${s.state}`,
        `${s.callsign} · ${flag}`,
        i === tunedIdx,
        `${s.callsign} · ${s.site}, ${s.state}`,
      );
    })
    .join("");
  return `
    <section class="card">
      <h2>NOAA Weather — pick a transmitter</h2>
      ${locRowHtml()}
      ${
        rows
          ? `<div class="stations">${rows}</div>
             <p class="note" style="margin:8px 0 0">Tuning picks the channel; the radio
             receives the strongest transmitter on it.</p>`
          : `<p class="note" style="margin:10px 0 0">Set your location to list nearby transmitters.</p>`
      }
    </section>`;
}

function fmWizardHtml(): string {
  const r = state.radio!;
  const mhz = (r.frequency_hz / 1e6).toFixed(1);
  const rows = state.fmNearby
    .map((s) =>
      stationRow(
        s.freq_hz,
        (s.freq_hz / 1e6).toFixed(1),
        `${s.call} — ${s.city}, ${s.state}`,
        `class ${s.class}${s.erp_kw ? ` · ${s.erp_kw} kW` : ""} · ${s.distance_mi.toFixed(0)} mi`,
        s.freq_hz === r.frequency_hz,
        `${s.call} · ${s.city}, ${s.state}`,
      ),
    )
    .join("");
  const tab = state.bandTab;
  return `
    <section class="card">
      <h2>FM Broadcast — tune a station</h2>
      <div class="tabs">
        <button class="tab${tab === "nearby" ? " on" : ""}" data-bandtab="nearby">Nearby</button>
        <button class="tab${tab === "manual" ? " on" : ""}" data-bandtab="manual">Manual</button>
      </div>
      ${
        tab === "nearby"
          ? `${locRowHtml()}
             ${rows ? `<div class="stations">${rows}</div>` : `<p class="note" style="margin:10px 0 0">Set your location to list nearby FM stations.</p>`}`
          : `<div class="dial">
               <button id="fm-down" class="secondary">−</button>
               <input id="fm-freq" type="text" inputmode="decimal" value="${mhz}" />
               <span class="dial-unit">MHz</span>
               <button id="fm-up" class="secondary">+</button>
               <button id="fm-go">Tune</button>
             </div>
             <p class="note" style="margin:8px 0 0">${FM_MIN_HZ / 1e6}–${FM_MAX_HZ / 1e6} MHz · 0.2 MHz steps · use Seek in “Now playing”.</p>`
      }
    </section>`;
}

function amWizardHtml(): string {
  const r = state.radio!;
  const khz = (r.frequency_hz / 1e3).toFixed(0);
  const rows = state.amNearby
    .map((s) =>
      stationRow(
        s.freq_hz,
        (s.freq_hz / 1e3).toFixed(0),
        `${s.call} — ${s.city}, ${s.state}`,
        `class ${s.class}${s.power_kw ? ` · ${s.power_kw} kW day` : ""} · ${s.distance_mi.toFixed(0)} mi`,
        s.freq_hz === r.frequency_hz,
        `${s.call} · ${s.city}, ${s.state}`,
      ),
    )
    .join("");
  const tab = state.bandTab;
  return `
    <section class="card">
      <h2>AM Radio — tune a station</h2>
      <div class="tabs">
        <button class="tab${tab === "nearby" ? " on" : ""}" data-bandtab="nearby">Nearby</button>
        <button class="tab${tab === "manual" ? " on" : ""}" data-bandtab="manual">Manual</button>
      </div>
      ${
        tab === "nearby"
          ? `${locRowHtml()}
             ${rows ? `<div class="stations">${rows}</div>` : `<p class="note" style="margin:10px 0 0">Set your location to list nearby AM stations.</p>`}`
          : `<div class="dial">
               <button id="am-down" class="secondary">−</button>
               <input id="am-freq" type="text" inputmode="decimal" value="${khz}" />
               <span class="dial-unit">kHz</span>
               <button id="am-up" class="secondary">+</button>
               <button id="am-go">Tune</button>
             </div>
             <p class="note" style="margin:8px 0 0">${AM_MIN_HZ / 1e3}–${AM_MAX_HZ / 1e3} kHz · 10 kHz steps · use Seek in “Now playing”.</p>`
      }
    </section>`;
}

function frsWizardHtml(): string {
  const r = state.radio!;
  const rows = FRS_CHANNELS.map((c) =>
    stationRow(
      c.freq_hz,
      `Ch ${c.channel}`,
      `${(c.freq_hz / 1e6).toFixed(4)} MHz`,
      `${c.shared_gmrs ? "FRS/GMRS shared" : "FRS only"} · ${c.max_power_w} W max`,
      c.freq_hz === r.frequency_hz,
      `Ch ${c.channel}`,
    ),
  ).join("");
  return `
    <section class="card">
      <h2>FRS — pick a channel</h2>
      <p class="note" style="margin:0 0 10px">Receive-only for now: this
      monitors the 22 fixed FRS channels (462/467 MHz), it doesn't key up a
      transmission. Anyone could be on any channel at any time — there's no
      station directory to browse, just the channel plan itself. Use Seek in
      “Now playing” to scan for the next active channel.</p>
      <div class="stations">${rows}</div>
    </section>`;
}

/** The amateur band whose edges bracket `hz`, or null if `hz` is out of band. */
function hamBandContaining(hz: number) {
  return HAM_BANDS.find((b) => hz >= b.loHz && hz <= b.hiHz) ?? null;
}

function hamWizardHtml(): string {
  const r = state.radio!;
  // The active band follows the tuned frequency when it's in a ham band, so
  // the tab bar and the dial never disagree; `state.hamBand` is only the
  // fallback (e.g. right after a mode switch, before the first tune).
  const band = hamBandContaining(r.frequency_hz) ?? hamBand(state.hamBand);
  const mhz = (r.frequency_hz / 1e6).toFixed(4);
  const seg = hamSegmentAt(band, r.frequency_hz);

  const tabs = HAM_BANDS.map(
    (b) =>
      `<button class="tab${b.id === band.id ? " on" : ""}" data-hamband="${b.id}">${b.name}</button>`,
  ).join("");

  const simplexRows = hamSimplexChannels(band)
    .map((s) =>
      stationRow(
        s.hz,
        `${(s.hz / 1e6).toFixed(4)}`,
        s.name,
        s.note ?? "FM simplex",
        s.hz === r.frequency_hz,
        `${s.name} · ${(s.hz / 1e6).toFixed(4)} MHz`,
      ),
    )
    .join("");

  const planRows = band.segments
    .map((s) => {
      const here = r.frequency_hz >= s.loHz && r.frequency_hz < s.hiHz;
      const lo = (s.loHz / 1e6).toFixed(3).replace(/\.?0+$/, "");
      const hi = (s.hiHz / 1e6).toFixed(3).replace(/\.?0+$/, "");
      return `<div class="ham-seg${here ? " here" : ""}${s.fm ? "" : " nofm"}" data-seg="${s.loHz} ${s.hiHz}">
        <span class="ham-seg-range">${lo}–${hi}</span>
        <span class="ham-seg-use">${esc(s.use)}</span>
      </div>`;
    })
    .join("");

  // Prefixed with "±" in the copy, so show the magnitude only.
  const offKHz = Math.abs(band.repeaterOffsetHz) / 1e3;
  const bandOffsetLabel =
    offKHz >= 1000 ? `${(offKHz / 1000).toFixed(offKHz % 1000 ? 1 : 0)} MHz` : `${offKHz} kHz`;

  const cfgTone = Number(r.mode_params.ctcss_hz ?? 0);
  const cfgDcs = Number(r.mode_params.dcs_code ?? 0);
  const cfgDcsInv = Number(r.mode_params.dcs_invert ?? 0) !== 0;
  const toneMonitor = Number(r.mode_params.ctcss_squelch ?? 1) === 0;
  const toneOpts = [
    `<option value="0"${cfgTone === 0 ? " selected" : ""}>CTCSS off</option>`,
    ...CTCSS_TONES.map(
      (t) =>
        `<option value="${t}"${Math.abs(t - cfgTone) < 0.05 ? " selected" : ""}>${t.toFixed(1)} Hz</option>`,
    ),
  ].join("");
  const dcsOpts = [
    `<option value="0"${cfgDcs === 0 ? " selected" : ""}>DCS off</option>`,
    ...DCS_CODES.map(
      (c) => `<option value="${c}"${c === cfgDcs ? " selected" : ""}>D${String(c).padStart(3, "0")}</option>`,
    ),
  ].join("");

  return `
    <section class="card">
      <h2>Amateur FM — ${band.name} (${band.rangeLabel})</h2>
      <div class="tabs ham-bands">${tabs}</div>
      <p class="note" style="margin:2px 0 10px">
        Open to <strong>all U.S. license classes</strong> (Technician and up) for FM
        voice — you hold Amateur Extra (KZ4AZ). Repeater offset on this band is
        conventionally <strong>±${bandOffsetLabel}</strong>. Receive-only for now:
        repeater input/tone TX comes in a later build. The band-plan slices
        below are the <strong>voluntary ARRL plan</strong>, not FCC sub-band
        rules — local coordinators refine them.
      </p>
      ${
        seg && !seg.fm
          ? `<p class="note warn" style="margin:0 0 10px">Heads up: ${(r.frequency_hz / 1e6).toFixed(4)} MHz
             is in a <strong>${esc(seg.use)}</strong> segment — normally CW/SSB/weak-signal, not FM.</p>`
          : ""
      }

      <div class="tabs" style="margin:6px 0 0">
        <button class="tab${state.hamView === "simplex" ? " on" : ""}" data-hamview="simplex">Simplex</button>
        <button class="tab${state.hamView === "repeater" ? " on" : ""}" data-hamview="repeater">Repeaters</button>
      </div>
      ${state.hamView === "repeater" ? hamRepeaterSection(band.id) : `
        <h3 class="ham-sub">Simplex &amp; calling</h3>
        <div class="stations">${simplexRows}</div>`}

      <h3 class="ham-sub">Manual tune</h3>
      <div class="dial">
        <button id="ham-down" class="secondary">−</button>
        <input id="ham-freq" type="text" inputmode="decimal" value="${mhz}" />
        <span class="dial-unit">MHz</span>
        <button id="ham-up" class="secondary">+</button>
        <button id="ham-go">Tune</button>
      </div>
      <p class="note" style="margin:8px 0 0">${(band.loHz / 1e6).toFixed(0)}–${(band.hiHz / 1e6).toFixed(0)} MHz ·
      5 kHz steps · ${seg ? esc(seg.use) : "out of the band plan"}.</p>

      <h3 class="ham-sub">Sub-audible squelch</h3>
      <div class="ham-ctcss">
        <select id="ham-ctcss">${toneOpts}</select>
        <select id="ham-dcs">${dcsOpts}</select>
        <label class="ham-ck"><input type="checkbox" id="ham-dcs-inv"${cfgDcsInv ? " checked" : ""} /> DCS inv</label>
        <label class="ham-ck"><input type="checkbox" id="ham-ctcss-mon"${toneMonitor ? " checked" : ""} /> monitor</label>
      </div>
      <p class="note" style="margin:6px 0 0">
        CTCSS on air: <span id="ham-scan">—</span>
        <button id="ham-ctcss-use" class="secondary" hidden>use it</button>
      </p>
      <p class="note" style="margin:4px 0 0">
        ${
          cfgTone > 0
            ? `Requiring CTCSS <strong>${cfgTone.toFixed(1)} Hz</strong>${toneMonitor ? " (monitor)" : ""} —
               <span id="ham-ctcss-det">…</span>.`
            : cfgDcs > 0
              ? `Requiring DCS <strong>D${String(cfgDcs).padStart(3, "0")}${cfgDcsInv ? "I" : "N"}</strong>${toneMonitor ? " (monitor)" : ""} —
                 <span id="ham-ctcss-det">…</span>. If it never locks on-air, toggle <em>DCS inv</em>.`
              : "Pick the CTCSS tone or DCS code a repeater needs to hear only its traffic. CTCSS and DCS are mutually exclusive."
        }
      </p>

      <h3 class="ham-sub">Band plan (voluntary)</h3>
      <div class="ham-plan">${planRows}</div>
    </section>`;
}

const RP_OFFSETS: [string, number][] = [
  ["−600 kHz", -600_000],
  ["+600 kHz", 600_000],
  ["−1 MHz", -1_000_000],
  ["−1.6 MHz", -1_600_000],
  ["−5 MHz", -5_000_000],
  ["+5 MHz", 5_000_000],
  ["−12 MHz", -12_000_000],
  ["−25 MHz", -25_000_000],
  ["simplex (0)", 0],
];

function toneSelect(id: string, sel: number): string {
  const opts = [
    `<option value="0"${sel === 0 ? " selected" : ""}>none</option>`,
    ...CTCSS_TONES.map(
      (t) => `<option value="${t}"${Math.abs(t - sel) < 0.05 ? " selected" : ""}>${t.toFixed(1)}</option>`,
    ),
  ].join("");
  return `<select id="${id}">${opts}</select>`;
}

function repeaterRow(rp: NearRepeater, tunedHz: number): string {
  const meta = [
    offsetLabel(rp.offset_hz),
    rp.tone_hz ? `T ${rp.tone_hz.toFixed(1)}` : null,
    rp.tsq_hz ? `TSQ ${rp.tsq_hz.toFixed(1)}` : null,
    rp.place || null,
    rp.distance_mi != null ? `${rp.distance_mi} mi` : null,
    rp.open ? null : "closed",
  ]
    .filter(Boolean)
    .join(" · ");
  return `<button class="station repeater${rp.output_hz === tunedHz ? " tuned" : ""}"
      data-rpid="${esc(rp.id)}" data-hz="${rp.output_hz}" data-label="${esc(rp.call)}">
    <span class="s-call">${esc(rp.call)}${rp.manual ? " ✎" : ""}</span>
    <span class="s-site">${(rp.output_hz / 1e6).toFixed(4)} MHz</span>
    <span class="s-meta">${esc(meta)}</span>
    ${rp.manual ? `<span class="rp-del" data-rpdel="${esc(rp.id)}" title="remove">✕</span>` : ""}
  </button>`;
}

function hamRepeaterSection(bandId: string): string {
  const list = state.repeaters;
  const tunedHz = state.radio!.frequency_hz;
  const rows = list.length
    ? list.map((rp) => repeaterRow(rp, tunedHz)).join("")
    : `<p class="note" style="margin:8px 0">No repeaters listed for this band nearby.${
        state.loc ? "" : " Set your location above to sort the nationwide list by distance."
      } Add one below.</p>`;

  const addForm = state.rpAddOpen
    ? `<div class="rp-add">
        <div class="rp-paste-row">
          <input id="rp-paste" type="text" placeholder="paste: 146.94 - 100.0  ·  442.100 +5 PL 131.8 W4ABC" />
          <button id="rp-paste-fill" class="secondary">Fill</button>
        </div>
        <div class="rp-add-grid">
          <label>Call <input id="rp-call" type="text" autocapitalize="characters" placeholder="W4XYZ" /></label>
          <label>Output MHz <input id="rp-output" type="text" inputmode="decimal" placeholder="146.940" /></label>
          <label>Offset <select id="rp-offset">${RP_OFFSETS.map(
            ([lbl, v]) => `<option value="${v}">${lbl}</option>`,
          ).join("")}</select></label>
          <label>Uplink tone ${toneSelect("rp-tone", 0)}</label>
          <label>Output tone ${toneSelect("rp-tsq", 0)}</label>
          <label>Place <input id="rp-place" type="text" placeholder="Gainesville, FL" /></label>
        </div>
        <div style="margin-top:8px; display:flex; gap:8px">
          <button id="rp-add-save">Save repeater</button>
          <button id="rp-add-cancel" class="secondary">Cancel</button>
        </div>
      </div>`
    : `<button id="rp-add-toggle" class="secondary" style="margin-top:10px">+ Add repeater</button>`;

  return `
    <h3 class="ham-sub">Repeaters — ${esc(bandId)}</h3>
    <p class="note" style="margin:4px 0 0">Tapping a repeater tunes its output and,
    if it transmits a tone, sets that as your receive tone squelch. Offset and
    uplink tone are stored for transmit (a later build).</p>
    <div class="stations">${rows}</div>
    ${addForm}`;
}

/** Tune a repeater's output frequency and apply its downlink tone (if any)
 *  as RX tone squelch. Remembers the repeater so the future TX path has its
 *  input frequency + uplink tone. */
async function tuneRepeater(rp: Repeater): Promise<void> {
  if (!state.client || !state.radio) return;
  const band = hamBandContaining(rp.output_hz);
  if (band && band.id !== state.hamBand) {
    localStorage.setItem("lanline.hamBand", band.id);
    setState({ hamBand: band.id });
  }
  try {
    let radio = await state.client.patchRadio({
      frequency_hz: rp.output_hz,
      mode_params: { ctcss_hz: rp.tsq_hz || 0, ctcss_squelch: 1 },
    });
    localStorage.setItem(freqKey("ham"), String(rp.output_hz));
    if (!radio.running) radio = await state.client.startRadio();
    setState({ radio, activeRepeater: rp });
    if (state.audio && state.audioState === "idle") void startAudio();
    logLine(
      `repeater ${rp.call} · ${(rp.output_hz / 1e6).toFixed(4)} ${offsetLabel(rp.offset_hz)}` +
        (rp.tsq_hz ? ` · RX tone ${rp.tsq_hz.toFixed(1)}` : ""),
    );
  } catch (e) {
    logLine(`repeater tune failed — ${(e as ApiError).message}`);
  }
}

/** Parse the "paste" box and pre-fill the add-repeater form fields. */
function fillRepeaterFromShorthand(): void {
  const g = <T extends HTMLElement>(id: string) => panels.querySelector<T>(id);
  const raw = g<HTMLInputElement>("#rp-paste")?.value ?? "";
  const p = parseRepeaterShorthand(raw);
  if (p.output_hz == null && p.call == null && p.tone_hz == null) {
    logLine("paste — couldn't read a frequency, tone or call from that");
    return;
  }
  const set = (id: string, v: string) => {
    const el = g<HTMLInputElement | HTMLSelectElement>(id);
    if (el) el.value = v;
  };
  if (p.output_hz != null) set("#rp-output", (p.output_hz / 1e6).toFixed(4).replace(/0+$/, "").replace(/\.$/, ""));
  if (p.call) set("#rp-call", p.call);
  if (p.tone_hz != null) set("#rp-tone", String(p.tone_hz));
  if (p.tsq_hz != null) set("#rp-tsq", String(p.tsq_hz));
  if (p.offset_hz != null) {
    // Snap to the closest preset the <select> offers.
    const closest = RP_OFFSETS.reduce((best, [, v]) =>
      Math.abs(v - p.offset_hz!) < Math.abs(best - p.offset_hz!) ? v : best, RP_OFFSETS[0]![1]);
    set("#rp-offset", String(closest));
  }
  logLine("paste — filled the form, review and save");
}

function saveManualRepeater(): void {
  const g = <T extends HTMLElement>(id: string) => panels.querySelector<T>(id);
  const call = (g<HTMLInputElement>("#rp-call")?.value ?? "").trim().toUpperCase();
  const outMhz = parseFloat(g<HTMLInputElement>("#rp-output")?.value ?? "");
  if (!call || !Number.isFinite(outMhz)) {
    logLine("add repeater — need a call sign and output frequency");
    return;
  }
  const output_hz = Math.round(outMhz * 1e6);
  const band = hamBandContaining(output_hz);
  if (!band) {
    logLine("add repeater — output frequency isn't in an amateur band");
    return;
  }
  const offRaw = g<HTMLSelectElement>("#rp-offset")?.value;
  const offset_hz = offRaw != null && offRaw !== "" ? Number(offRaw) : conventionalOffsetHz(output_hz);
  const rp: Repeater = {
    id: `m-${call}-${output_hz}`,
    call,
    output_hz,
    offset_hz,
    tone_hz: Number(g<HTMLSelectElement>("#rp-tone")?.value ?? 0),
    tsq_hz: Number(g<HTMLSelectElement>("#rp-tsq")?.value ?? 0),
    lat: state.loc?.lat ?? 0,
    lon: state.loc?.lon ?? 0,
    place: (g<HTMLInputElement>("#rp-place")?.value ?? "").trim(),
    band: band.id,
    open: true,
    manual: true,
  };
  const manualRepeaters = [...state.manualRepeaters.filter((x) => x.id !== rp.id), rp];
  localStorage.setItem(RP_MANUAL_KEY, JSON.stringify(manualRepeaters));
  setState({ manualRepeaters, rpAddOpen: false });
  logLine(`saved repeater ${call} ${(output_hz / 1e6).toFixed(4)}`);
  void rebuildRepeaterList();
}

function deleteManualRepeater(id: string): void {
  const manualRepeaters = state.manualRepeaters.filter((x) => x.id !== id);
  localStorage.setItem(RP_MANUAL_KEY, JSON.stringify(manualRepeaters));
  setState({ manualRepeaters });
  void rebuildRepeaterList();
}

/** Apply the ham sub-audible squelch controls (CTCSS tone / DCS code / invert
 *  / monitor). CTCSS and DCS are mutually exclusive — the last one changed
 *  wins, and the other's <select> is reset in the DOM. */
async function applyHamSquelch(changed?: "ctcss" | "dcs"): Promise<void> {
  if (!state.client || !state.radio) return;
  const g = <T extends HTMLElement>(id: string) => panels.querySelector<T>(id);
  const ctcssSel = g<HTMLSelectElement>("#ham-ctcss");
  const dcsSel = g<HTMLSelectElement>("#ham-dcs");
  let ctcss_hz = ctcssSel ? Number(ctcssSel.value) : 0;
  let dcs_code = dcsSel ? Number(dcsSel.value) : 0;
  if (changed === "ctcss" && ctcss_hz > 0) {
    dcs_code = 0;
    if (dcsSel) dcsSel.value = "0";
  } else if (changed === "dcs" && dcs_code > 0) {
    ctcss_hz = 0;
    if (ctcssSel) ctcssSel.value = "0";
  } else if (ctcss_hz > 0) {
    dcs_code = 0;
  }
  const dcs_invert = g<HTMLInputElement>("#ham-dcs-inv")?.checked ? 1 : 0;
  const monitor = g<HTMLInputElement>("#ham-ctcss-mon")?.checked ? 0 : 1;
  try {
    const radio = await state.client.patchRadio({
      mode_params: {
        ctcss_hz,
        ctcss_squelch: monitor,
        dcs_code,
        dcs_invert,
        dcs_squelch: monitor,
      },
    });
    setState({ radio });
    logLine(
      ctcss_hz > 0
        ? `CTCSS ${ctcss_hz.toFixed(1)} Hz${monitor ? "" : " · monitor"}`
        : dcs_code > 0
          ? `DCS D${String(dcs_code).padStart(3, "0")}${dcs_invert ? "I" : "N"}${monitor ? "" : " · monitor"}`
          : "sub-audible squelch off",
    );
  } catch (e) {
    logLine(`squelch set failed — ${(e as ApiError).message}`);
  }
}

function hamStep(hz: number, dir: 1 | -1): number {
  const band = hamBandContaining(hz) ?? hamBand(state.hamBand);
  const next = Math.round(hz + dir * 5_000);
  return Math.min(band.hiHz, Math.max(band.loHz, next));
}

function hamManualGo(): void {
  const raw = panels.querySelector<HTMLInputElement>("#ham-freq")?.value ?? "";
  const mhz = parseFloat(raw);
  if (!Number.isFinite(mhz)) return;
  const want = Math.round(mhz * 1e6);
  // If the typed frequency is in a *different* amateur band, jump there;
  // otherwise clamp to the current band's edges.
  const band =
    hamBandContaining(want) ?? hamBandContaining(state.radio!.frequency_hz) ?? hamBand(state.hamBand);
  if (band.id !== state.hamBand) {
    localStorage.setItem("lanline.hamBand", band.id);
    setState({ hamBand: band.id });
    if (state.hamView === "repeater") void rebuildRepeaterList(band.id);
  }
  void tuneFrequency(Math.min(band.hiHz, Math.max(band.loHz, want)));
}

/** "next pass in 3h12m · 58° max" / "no pass in the next 48h" / a prompt to
 *  set a location — pass times are predicted locally via SGP4 against the
 *  bundled TLE (see `apt.ts`), not fetched from a live tracking service. */
function aptPassLabel(name: string): string {
  if (!state.loc) return "set location for pass times";
  const passes = state.aptPasses?.[name];
  if (!passes) return "loading pass times…";
  if (passes.length === 0) return "no pass in the next 48h";
  const p = passes[0]!;
  const mins = Math.max(0, Math.round((p.aos - Date.now()) / 60_000));
  const when = mins === 0 ? "now" : mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h${mins % 60}m`;
  return `next pass in ${when} · ${p.max_elevation_deg}° max`;
}

/** Just the satellite rows — refreshed on its own each poll (see
 *  `patchLive`) so the "next pass in Xm" countdown ticks down without
 *  rebuilding the whole panel or recomputing SGP4. */
function aptSatRowsInner(): string {
  const r = state.radio!;
  return APT_SATELLITES.map((s) =>
    stationRow(s.freq_hz, (s.freq_hz / 1e6).toFixed(4), s.name, aptPassLabel(s.name), s.freq_hz === r.frequency_hz, s.name),
  ).join("");
}

function aptWizardHtml(): string {
  const r = state.radio!;
  const mhz = (r.frequency_hz / 1e6).toFixed(4);
  const tab = state.bandTab;
  return `
    <section class="card">
      <h2>NOAA APT — weather satellite image</h2>
      <p class="note" style="margin:0 0 10px">Unlike AM/FM/ADS-B/AIS, this is a
      real-time downlink from one specific satellite — reception only works
      during an actual overhead pass. Pass times below are predicted locally
      from current orbital elements (SGP4 against a TLE fetched ahead of
      time — no live tracking service, works offline); accuracy drifts as
      the TLE ages, and a low max elevation or an obstructed horizon can
      still leave a "pass" unreceivable.</p>
      <div class="tabs">
        <button class="tab${tab === "nearby" ? " on" : ""}" data-bandtab="nearby">Satellites</button>
        <button class="tab${tab === "manual" ? " on" : ""}" data-bandtab="manual">Manual</button>
      </div>
      ${
        tab === "nearby"
          ? `${locRowHtml()}<div class="stations" id="apt-sat-rows">${aptSatRowsInner()}</div>`
          : `<div class="dial">
               <input id="apt-freq" type="text" inputmode="decimal" value="${mhz}" />
               <span class="dial-unit">MHz</span>
               <button id="apt-go">Tune</button>
             </div>
             <p class="note" style="margin:8px 0 0">${APT_MIN_HZ / 1e6}–${APT_MAX_HZ / 1e6} MHz.</p>`
      }
      <div class="apt-wrap" style="margin-top:12px">
        <canvas id="apt-canvas" class="apt-canvas"></canvas>
      </div>
      <p class="note" id="apt-status" style="margin:8px 0 0">${aptStatusInner()}</p>
    </section>`;
}

function aptStatusInner(): string {
  const a = state.apt;
  if (!a || a.lines === 0) {
    return state.radio?.running
      ? "Listening — no scan lines decoded yet."
      : "Start the receiver, then wait for a pass.";
  }
  return `${a.lines} line${a.lines === 1 ? "" : "s"} · ${a.width}×${a.height} px · sync quality ${a.sync_quality.toFixed(1)}${a.sync_quality > 5 ? " (locked)" : ""}`;
}

async function refreshAptImage(): Promise<void> {
  if (!state.client) return;
  try {
    aptImg = await state.client.aptImage(undefined, 8_000);
    drawAptImage();
  } catch {
    /* best effort — keep showing the last successfully decoded frame */
  }
}

function drawAptImage(): void {
  const canvas = panels.querySelector<HTMLCanvasElement>("#apt-canvas");
  if (!canvas || !aptImg || aptImg.width === 0 || aptImg.height === 0) return;
  const { width, height, pixels } = aptImg;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const img = ctx.createImageData(width, height);
  for (let i = 0; i < pixels.length; i++) {
    const v = pixels[i];
    const o = i * 4;
    img.data[o] = v;
    img.data[o + 1] = v;
    img.data[o + 2] = v;
    img.data[o + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
}

// --- radar scope (shared by ADS-B + AIS) -------------------------------

const SCOPE_RANGES: (number | "auto")[] = ["auto", 5, 10, 20, 40, 80, 160, 320];
/** Screen positions from the last scope draw, for click hit-testing. */
let scopePlot: { id: string; x: number; y: number }[] = [];

interface ScopeContact {
  id: string;
  label: string;
  sub: string;
  lat: number | null;
  lon: number | null;
  course: number | null;
  trail: [number, number][];
  kind: "air" | "sea" | "land";
}

interface ScopeData {
  contacts: ScopeContact[];
  receiver: [number, number] | null;
  title: string;
  empty: string;
}

/** Normalize the active mode's tracks into scope contacts, or null. */
function activeScope(): ScopeData | null {
  if (state.radio?.mode === "adsb") {
    const a = state.adsb;
    if (!a) return null;
    return {
      receiver: a.receiver,
      title: "ADS-B — 1090 MHz aircraft",
      empty: "Listening for aircraft…",
      contacts: a.aircraft.map((ac) => ({
        id: ac.icao,
        label: ac.callsign || ac.icao.toUpperCase(),
        sub: `${altLabel(ac.altitude_ft)}${
          ac.vertical_rate_fpm != null && Math.abs(ac.vertical_rate_fpm) >= 200
            ? ac.vertical_rate_fpm > 0
              ? " ↑"
              : " ↓"
            : ""
        } · ${ac.ground_speed_kt != null ? `${ac.ground_speed_kt.toFixed(0)} kt` : "—"}${
          ac.distance_nm != null ? ` · ${ac.distance_nm.toFixed(0)} NM` : ""
        }${ac.bearing_deg != null ? ` ${ac.bearing_deg.toFixed(0)}°` : ""}`,
        lat: ac.lat,
        lon: ac.lon,
        course: ac.track_deg,
        trail: ac.trail,
        kind: "air",
      })),
    };
  }
  if (state.radio?.mode === "ais") {
    const a = state.ais;
    if (!a) return null;
    return {
      receiver: a.receiver,
      title: "AIS — 161.975 / 162.025 MHz vessels",
      empty: "Listening for vessels…",
      contacts: a.vessels.map((v) => ({
        id: String(v.mmsi),
        label: v.name || v.callsign || String(v.mmsi),
        sub: `${v.ship_type_label ?? (v.aid ? "AtoN" : v.class_b ? "Class B" : "vessel")}${
          v.sog_kt != null ? ` · ${v.sog_kt.toFixed(1)} kt` : ""
        }${v.cog_deg != null ? ` ${v.cog_deg.toFixed(0)}°` : ""}${
          v.distance_nm != null ? ` · ${v.distance_nm.toFixed(1)} NM` : ""
        }`,
        lat: v.lat,
        lon: v.lon,
        course: v.cog_deg ?? v.heading_deg,
        trail: v.trail,
        kind: "sea",
      })),
    };
  }
  if (state.radio?.mode === "aprs") {
    const a = state.aprs;
    if (!a) return null;
    return {
      receiver: a.receiver,
      title: "APRS — 144.390 MHz packet",
      empty: "Listening for packets…",
      contacts: a.stations.map((s) => ({
        id: s.call,
        label: `${s.symbol ? aprsSymbolGlyph(s.symbol) + " " : ""}${s.call}`,
        sub: [
          s.speed_kt != null && s.speed_kt > 0 ? `${s.speed_kt.toFixed(0)} kt` : null,
          s.course_deg != null && s.speed_kt ? `${s.course_deg.toFixed(0)}°` : null,
          s.distance_km != null ? `${s.distance_km.toFixed(1)} km` : null,
          s.comment || s.last_message || null,
        ]
          .filter(Boolean)
          .join(" · ")
          .slice(0, 80) || `${s.packets} pkt`,
        lat: s.lat,
        lon: s.lon,
        course: s.course_deg,
        trail: s.trail,
        kind: "land",
      })),
    };
  }
  return null;
}

function scopeWizardHtml(): string {
  const s = activeScope();
  const rangeOpts = SCOPE_RANGES.map(
    (r) =>
      `<option value="${r}"${r === state.scopeRangeNm ? " selected" : ""}>${
        r === "auto" ? "Auto range" : `${r} NM`
      }</option>`,
  ).join("");
  return `
    <section class="card">
      <h2>${esc(s?.title ?? "Radar scope")}</h2>
      ${
        state.loc
          ? ""
          : `<p class="note" style="margin:0 0 10px">Set a location to centre the scope
             on you and show range/bearing.</p>${locRowHtml()}`
      }
      <div class="scope-wrap">
        <canvas id="scope" class="scope"></canvas>
      </div>
      <div class="connect-row" style="margin-top:10px">
        <select id="scope-range" class="dial-select">${rangeOpts}</select>
        <button id="scope-recenter" class="secondary">Recenter</button>
      </div>
      <div id="scope-list" class="ac-list">${scopeListInner()}</div>
    </section>`;
}

function scopeListInner(): string {
  const s = activeScope();
  if (!s || s.contacts.length === 0) {
    return `<p class="note" style="margin:10px 0 0">${
      state.radio?.running ? esc(s?.empty ?? "Listening…") : "Start the receiver to track contacts."
    }</p>`;
  }
  const sel = state.scopeSel;
  const mode = state.radio?.mode;
  const detailFor = (id: string): string =>
    mode === "adsb" ? flightDetailHtml(id) : mode === "ais" ? vesselDetailHtml(id) : "";
  const rows = s.contacts
    .map(
      (c) =>
        `<button class="ac-row${c.id === sel ? " tuned" : ""}${c.lat == null ? " noloc" : ""}" data-cid="${esc(c.id)}">
        <span class="s-call">${esc(c.label)}</span>
        <span class="s-site">${esc(c.sub)}</span>
      </button>${c.id === sel ? detailFor(c.id) : ""}`,
    )
    .join("");
  return `<div class="stations ac-table">${rows}</div>`;
}

/** Expanded internet lookup panel under a selected ADS-B row. Reads
 *  `state.acInfo` (populated by `requestFlightInfo`); renders nothing until a
 *  request has been kicked. */
function flightDetailHtml(icao: string): string {
  if (!state.server?.capabilities.includes("flight-lookup")) return "";
  const info = state.acInfo[icao];
  if (!info) return "";
  if (info === "loading") {
    return `<div class="ac-detail"><span class="note">looking up…</span></div>`;
  }
  if (!info.available) {
    const msg =
      info.reason === "offline"
        ? "no internet — flight data unavailable"
        : info.reason === "disabled"
          ? "flight lookup is disabled on this server"
          : "no flight data found";
    return `<div class="ac-detail"><span class="note">${esc(msg)}</span></div>`;
  }

  const airport = (a: NonNullable<FlightInfo["route"]>["origin"]): string => {
    const code = a.icao || a.iata || "";
    const place = a.city || a.name || "";
    return esc([code, place].filter(Boolean).join(" ") || "?");
  };
  const line1 = [info.registration, info.icao_type || info.aircraft_type, info.manufacturer]
    .filter(Boolean)
    .map((x) => esc(x as string))
    .join(" · ");
  const owner = [info.owner, info.owner_country].filter(Boolean).map((x) => esc(x as string)).join(", ");
  const routeLine = info.route
    ? `${info.airline ? esc(info.airline) + " · " : ""}${airport(info.route.origin)} → ${airport(
        info.route.destination,
      )}`
    : "";

  return `<div class="ac-detail">
    ${
      info.photo_thumb_url
        ? `<img class="ac-photo" src="${esc(info.photo_thumb_url)}" alt="" loading="lazy"
             referrerpolicy="no-referrer" onerror="this.remove()" />`
        : ""
    }
    ${line1 ? `<div class="ac-detail-line">${line1}</div>` : ""}
    ${owner ? `<div class="ac-detail-line note">${owner}</div>` : ""}
    ${routeLine ? `<div class="ac-detail-line">${routeLine}</div>` : ""}
    <div class="ac-detail-src note">via adsbdb.com</div>
  </div>`;
}

/** Expanded internet lookup panel under a selected AIS row. Reads
 *  `state.vesselInfo` (populated by `requestVesselInfo`). */
function vesselDetailHtml(mmsi: string): string {
  if (!state.server?.capabilities.includes("vessel-lookup")) return "";
  const info = state.vesselInfo[mmsi];
  if (!info) return "";
  if (info === "loading") {
    return `<div class="ac-detail"><span class="note">looking up…</span></div>`;
  }
  if (!info.available) {
    const msg =
      info.reason === "offline"
        ? "no internet — vessel data unavailable"
        : info.reason === "disabled"
          ? "vessel lookup is disabled on this server"
          : "no vessel data found";
    return `<div class="ac-detail"><span class="note">${esc(msg)}</span></div>`;
  }

  const j = (parts: (string | number | null | undefined)[]): string =>
    parts.filter((x) => x != null && x !== "").map((x) => esc(String(x))).join(" · ");
  const line1 = j([
    info.name,
    info.ship_type,
    info.flag,
  ]);
  const line2 = j([
    info.imo ? `IMO ${info.imo}` : null,
    info.gross_tonnage ? `${info.gross_tonnage.toLocaleString()} GT` : null,
    info.year_built ? `built ${info.year_built}` : null,
  ]);
  const dims = j([
    info.length_m ? `${info.length_m} × ${info.beam_m ?? "?"} m` : null,
    info.draught_m ? `${info.draught_m.toFixed(1)} m draught` : null,
  ]);
  const eta =
    info.eta && info.eta > 0
      ? ` · ETA ${new Date(info.eta * 1000).toLocaleString([], {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}`
      : "";
  const dest = info.destination ? `→ ${esc(info.destination)}${eta}` : "";

  return `<div class="ac-detail">
    ${
      info.photo_url
        ? `<img class="ac-photo" src="${esc(info.photo_url)}" alt="" loading="lazy"
             referrerpolicy="no-referrer" onerror="this.remove()" />`
        : ""
    }
    ${line1 ? `<div class="ac-detail-line">${line1}</div>` : ""}
    ${line2 ? `<div class="ac-detail-line note">${line2}</div>` : ""}
    ${dims ? `<div class="ac-detail-line note">${dims}</div>` : ""}
    ${dest ? `<div class="ac-detail-line">${dest}</div>` : ""}
    <div class="ac-detail-src note">via vesselfinder.com</div>
  </div>`;
}

/** Centre + NM-per-pixel for the current scope, or null if nothing to show. */
function scopeGeometry(cw: number): {
  lat0: number;
  lon0: number;
  rangeNm: number;
  pxPerNm: number;
} | null {
  const s = activeScope();
  if (!s) return null;
  const withPos = s.contacts.filter((c) => c.lat != null && c.lon != null);
  let lat0: number, lon0: number;
  if (s.receiver) {
    [lat0, lon0] = s.receiver;
  } else if (state.loc) {
    lat0 = state.loc.lat;
    lon0 = state.loc.lon;
  } else if (withPos.length) {
    lat0 = withPos.reduce((a, c) => a + c.lat!, 0) / withPos.length;
    lon0 = withPos.reduce((a, c) => a + c.lon!, 0) / withPos.length;
  } else {
    return null;
  }

  let rangeNm: number;
  if (state.scopeRangeNm === "auto") {
    let max = 4;
    for (const c of withPos) {
      const { north, east } = offsetNm(lat0, lon0, c.lat!, c.lon!);
      max = Math.max(max, Math.hypot(north, east));
    }
    rangeNm = Math.max(2, Math.ceil((max * 1.15) / 2) * 2);
  } else {
    rangeNm = state.scopeRangeNm;
  }
  const pxPerNm = (cw / 2 - 12) / rangeNm;
  return { lat0, lon0, rangeNm, pxPerNm };
}

function drawScope(): void {
  const canvas = panels.querySelector<HTMLCanvasElement>("#scope");
  if (!canvas) return;
  const cssW = canvas.clientWidth || 320;
  const dpr = window.devicePixelRatio || 1;
  const size = Math.round(cssW * dpr);
  if (canvas.width !== size) {
    canvas.width = size;
    canvas.height = size;
  }
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.save();
  ctx.scale(dpr, dpr);
  const cw = cssW;
  const cx = cw / 2;
  const cy = cw / 2;

  const css = getComputedStyle(document.body);
  const ink = css.getPropertyValue("--ink").trim() || "#e8eaed";
  const muted = css.getPropertyValue("--muted").trim() || "#9aa3af";
  const accent = css.getPropertyValue("--accent").trim() || "#60a5fa";

  ctx.clearRect(0, 0, cw, cw);
  ctx.fillStyle = "rgba(10, 20, 12, 0.92)";
  ctx.fillRect(0, 0, cw, cw);

  const geo = scopeGeometry(cw);
  ctx.strokeStyle = "rgba(120,160,130,0.35)";
  ctx.fillStyle = muted;
  ctx.font = "10px ui-monospace, monospace";
  ctx.lineWidth = 1;

  // range rings
  const rings = 4;
  for (let i = 1; i <= rings; i++) {
    const rr = ((cw / 2 - 12) * i) / rings;
    ctx.beginPath();
    ctx.arc(cx, cy, rr, 0, Math.PI * 2);
    ctx.stroke();
    if (geo) {
      ctx.fillText(`${((geo.rangeNm * i) / rings).toFixed(0)}`, cx + 3, cy - rr + 11);
    }
  }
  // cross-hairs + N
  ctx.beginPath();
  ctx.moveTo(cx, 8);
  ctx.lineTo(cx, cw - 8);
  ctx.moveTo(8, cy);
  ctx.lineTo(cw - 8, cy);
  ctx.stroke();
  ctx.fillStyle = muted;
  ctx.fillText("N", cx + 4, 12);

  // receiver
  ctx.fillStyle = accent;
  ctx.beginPath();
  ctx.arc(cx, cy, 3, 0, Math.PI * 2);
  ctx.fill();

  scopePlot = [];
  const s = activeScope();
  if (!geo || !s) {
    ctx.fillStyle = muted;
    ctx.fillText(
      s ? "waiting for positions…" : "start the receiver",
      cx - 44,
      cy + cw / 4,
    );
    ctx.restore();
    return;
  }

  for (const c of s.contacts) {
    if (c.lat == null || c.lon == null) continue;
    const { north, east } = offsetNm(geo.lat0, geo.lon0, c.lat, c.lon);
    const x = cx + east * geo.pxPerNm;
    const y = cy - north * geo.pxPerNm;
    if (x < 0 || x > cw || y < 0 || y > cw) continue;
    const selected = c.id === state.scopeSel;
    scopePlot.push({ id: c.id, x, y });

    if (c.trail.length > 1) {
      ctx.strokeStyle = selected ? accent : "rgba(140,180,150,0.5)";
      ctx.beginPath();
      c.trail.forEach(([tlat, tlon], k) => {
        const o = offsetNm(geo.lat0, geo.lon0, tlat, tlon);
        const tx = cx + o.east * geo.pxPerNm;
        const ty = cy - o.north * geo.pxPerNm;
        if (k === 0) ctx.moveTo(tx, ty);
        else ctx.lineTo(tx, ty);
      });
      ctx.stroke();
    }

    ctx.save();
    ctx.translate(x, y);
    ctx.rotate(((c.course ?? 0) * Math.PI) / 180);
    ctx.fillStyle = selected
      ? accent
      : c.kind === "sea"
        ? "#7fc8ff"
        : c.kind === "land"
          ? "#f5c26b"
          : "#8fe0a0";
    if (c.course != null && c.kind === "air") {
      ctx.beginPath();
      ctx.moveTo(0, -6);
      ctx.lineTo(4, 5);
      ctx.lineTo(0, 2);
      ctx.lineTo(-4, 5);
      ctx.closePath();
      ctx.fill();
    } else if (c.course != null) {
      // vessel: diamond + heading stick
      ctx.beginPath();
      ctx.moveTo(0, -4);
      ctx.lineTo(3, 0);
      ctx.lineTo(0, 4);
      ctx.lineTo(-3, 0);
      ctx.closePath();
      ctx.fill();
      ctx.strokeStyle = ctx.fillStyle as string;
      ctx.beginPath();
      ctx.moveTo(0, 0);
      ctx.lineTo(0, -9);
      ctx.stroke();
    } else {
      ctx.beginPath();
      ctx.arc(0, 0, 3, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();

    ctx.fillStyle = selected ? accent : ink;
    ctx.font = selected ? "bold 10px ui-monospace, monospace" : "10px ui-monospace, monospace";
    ctx.fillText(c.label, x + 7, y - 1);
  }
  ctx.restore();
}

function onScopeClick(ev: MouseEvent): void {
  const canvas = panels.querySelector<HTMLCanvasElement>("#scope");
  if (!canvas || scopePlot.length === 0) return;
  const rect = canvas.getBoundingClientRect();
  const px = ev.clientX - rect.left;
  const py = ev.clientY - rect.top;
  let best: { id: string; d: number } | null = null;
  for (const p of scopePlot) {
    const d = Math.hypot(p.x - px, p.y - py);
    if (d < 16 && (!best || d < best.d)) best = { id: p.id, d };
  }
  const next = best ? (best.id === state.scopeSel ? null : best.id) : null;
  setState({ scopeSel: next });
  if (next) enrichSelected(next);
}

/** Kick the right internet lookup for a just-selected scope contact. */
function enrichSelected(id: string): void {
  if (state.radio?.mode === "adsb") requestFlightInfo(id);
  else if (state.radio?.mode === "ais") requestVesselInfo(id);
}

/** Fetch vessel enrichment (flag / tonnage / year / photo) for a selected
 *  AIS contact via the server's vesselfinder proxy. No-op unless we're in
 *  `ais` mode, the server advertises `vessel-lookup`, and we don't already
 *  have it. Retries once while the server reports the lookup `pending`. */
function requestVesselInfo(mmsi: string): void {
  if (state.radio?.mode !== "ais") return;
  if (!state.server?.capabilities.includes("vessel-lookup")) return;
  if (state.vesselInfo[mmsi]) return;
  state.vesselInfo[mmsi] = "loading";
  setState({ vesselInfo: state.vesselInfo });
  const attempt = (retriesLeft: number): void => {
    if (!state.client) return;
    state.client
      .vesselInfo(mmsi)
      .then((info) => {
        if (info.reason === "pending" && retriesLeft > 0) {
          window.setTimeout(() => attempt(retriesLeft - 1), 1500);
          return;
        }
        state.vesselInfo[mmsi] = info;
        setState({ vesselInfo: state.vesselInfo });
      })
      .catch(() => {
        state.vesselInfo[mmsi] = { available: false, reason: "offline" };
        setState({ vesselInfo: state.vesselInfo });
      });
  };
  attempt(3);
}

/** Fetch internet enrichment (tail / type / owner / route) for a selected
 *  ADS-B contact via the server's adsbdb proxy. No-op unless we're in `adsb`
 *  mode, the server advertises `flight-lookup`, and we don't already have it.
 *  Retries a couple of times while the server reports the lookup `pending`
 *  (another client is fetching the same aircraft). */
function requestFlightInfo(icao: string): void {
  if (state.radio?.mode !== "adsb") return;
  if (!state.server?.capabilities.includes("flight-lookup")) return;
  if (state.acInfo[icao]) return;
  const callsign = state.adsb?.aircraft.find((a) => a.icao === icao)?.callsign ?? null;
  state.acInfo[icao] = "loading";
  setState({ acInfo: state.acInfo });
  const attempt = (retriesLeft: number): void => {
    if (!state.client) return;
    state.client
      .flightInfo(icao, callsign)
      .then((info) => {
        if (info.reason === "pending" && retriesLeft > 0) {
          window.setTimeout(() => attempt(retriesLeft - 1), 1500);
          return;
        }
        state.acInfo[icao] = info;
        setState({ acInfo: state.acInfo });
      })
      .catch(() => {
        state.acInfo[icao] = { available: false, reason: "offline" };
        setState({ acInfo: state.acInfo });
      });
  };
  attempt(3);
}

function toneWizardHtml(): string {
  const p = state.radio?.mode_params ?? {};
  return `
    <section class="card">
      <h2>Debug Tone</h2>
      <p class="note">A synthesized ${Number(p.tone_hz ?? 440).toFixed(0)} Hz tone at
      ${Number(p.level_dbfs ?? -12).toFixed(0)} dBFS — no SDR needed. Use it to verify the audio path.</p>
    </section>`;
}

function nowPlayingInner(): string {
  const r = state.radio!;
  const st = state.status;
  const meta = MODE_META[r.mode];

  if (r.mode === "adsb") {
    const a = state.adsb;
    return `
      <h2>Now tracking</h2>
      <div class="mode-line">
        <span class="mode">${esc(meta?.label ?? "ADS-B")}</span>
        <span class="freq">1090 MHz</span>
        <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "receiving" : "stopped"}</span>
      </div>
      <div class="grid">
        ${kv("Aircraft", a ? String(a.aircraft_count) : "—")}
        ${kv("With position", a ? String(a.with_position) : "—")}
        ${kv("Messages", a ? a.messages.toLocaleString() : "—")}
        ${kv("Message rate", a ? `${a.message_rate.toFixed(0)}/s` : "—")}
        ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
        ${kv("Reference", a?.receiver ? `${a.receiver[0].toFixed(3)}, ${a.receiver[1].toFixed(3)}` : "not set")}
      </div>
      <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
        <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
          ${r.running ? "Stop receiver" : "Start receiver"}
        </button>
        <button id="radio-options" class="secondary">⚙ Radio options</button>
      </div>`;
  }

  if (r.mode === "ais") {
    const a = state.ais;
    return `
      <h2>Now tracking</h2>
      <div class="mode-line">
        <span class="mode">${esc(meta?.label ?? "AIS")}</span>
        <span class="freq">161.975 / 162.025 MHz</span>
        <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "receiving" : "stopped"}</span>
      </div>
      <div class="grid">
        ${kv("Vessels", a ? String(a.vessel_count) : "—")}
        ${kv("With position", a ? String(a.with_position) : "—")}
        ${kv("Messages", a ? a.messages.toLocaleString() : "—")}
        ${kv("Message rate", a ? `${a.message_rate.toFixed(1)}/s` : "—")}
        ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
        ${kv("Reference", a?.receiver ? `${a.receiver[0].toFixed(3)}, ${a.receiver[1].toFixed(3)}` : "not set")}
      </div>
      <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
        <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
          ${r.running ? "Stop receiver" : "Start receiver"}
        </button>
        <button id="radio-options" class="secondary">⚙ Radio options</button>
      </div>`;
  }

  if (r.mode === "aprs") {
    const a = state.aprs;
    const std = Math.abs(r.frequency_hz - 144_390_000) < 1;
    return `
      <h2>Now tracking</h2>
      <div class="mode-line">
        <span class="mode">${esc(meta?.label ?? "APRS")}</span>
        <span class="freq">${freqLabel(r.mode, r.frequency_hz)}${std ? " · 2 m" : ""}</span>
        <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "receiving" : "stopped"}</span>
      </div>
      <div class="grid">
        ${kv("Stations", a ? String(a.station_count) : "—")}
        ${kv("With position", a ? String(a.with_position) : "—")}
        ${kv("Packets", a ? a.packets.toLocaleString() : "—")}
        ${kv("Packet rate", a ? `${a.packet_rate.toFixed(1)}/s` : "—")}
        ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
        ${kv("Reference", a?.receiver ? `${a.receiver[0].toFixed(3)}, ${a.receiver[1].toFixed(3)}` : "not set")}
      </div>
      <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
        <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
          ${r.running ? "Stop receiver" : "Start receiver"}
        </button>
        <button id="radio-options" class="secondary">⚙ Radio options</button>
      </div>`;
  }

  if (r.mode === "apt") {
    const a = state.apt;
    const sat = APT_SATELLITES.find((s) => s.freq_hz === r.frequency_hz);
    return `
      <h2>Now receiving</h2>
      <div class="mode-line">
        <span class="mode">${esc(meta?.label ?? "NOAA APT")}</span>
        <span class="freq">${freqLabel(r.mode, r.frequency_hz)}${sat ? ` · ${esc(sat.name)}` : ""}</span>
        <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "receiving" : "stopped"}</span>
      </div>
      <div class="grid">
        ${kv("Scan lines", a ? String(a.lines) : "—")}
        ${kv("Image size", a && a.lines > 0 ? `${a.width}×${a.height} px` : "—")}
        ${kv("Sync quality", a && a.lines > 0 ? a.sync_quality.toFixed(1) : "—")}
        ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
      </div>
      <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
        <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
          ${r.running ? "Stop receiver" : "Start receiver"}
        </button>
        <button id="radio-options" class="secondary">⚙ Radio options</button>
      </div>`;
  }

  const ident = tunedStationLabel();
  const rp = r.mode === "ham" && state.activeRepeater?.output_hz === r.frequency_hz ? state.activeRepeater : null;
  const rpNote = rp
    ? `via ${rp.call} · input ${((rp.output_hz + rp.offset_hz) / 1e6).toFixed(4)} (${offsetLabel(rp.offset_hz)})` +
      (rp.tone_hz ? ` · uplink ${rp.tone_hz.toFixed(1)} Hz` : "") +
      (rp.place ? ` · ${rp.place}` : "")
    : null;
  const canSeek = r.mode === "wbfm" || r.mode === "nbfm" || r.mode === "am" || r.mode === "frs";
  const canScan = canSeek || r.mode === "ham";
  const scan = st?.scan;
  const scanning = scan?.active ?? false;
  const shownHz = scanning && scan?.frequency_hz ? scan.frequency_hz : r.frequency_hz;
  const scanNote = scanning
    ? scan!.parked
      ? `▶ parked on ${esc(scan!.label ?? freqLabel(r.mode, shownHz))}`
      : `⏱ scanning ${scan!.index + 1}/${scan!.total} · ${esc(scan!.label ?? "…")}`
    : null;
  return `
    <h2>Now playing</h2>
    <div class="mode-line">
      <span class="mode">${esc(meta?.label ?? r.mode)}</span>
      <span class="freq">${freqLabel(r.mode, shownHz)}</span>
      <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "running" : "stopped"}</span>
    </div>
    ${scanNote ? `<p class="note ${scan!.parked ? "" : "warn"}" style="margin:0 0 12px">${scanNote}</p>` : ""}
    ${!scanning && rpNote ? `<p class="note" style="margin:0 0 12px">${esc(rpNote)}</p>` : !scanning && ident ? `<p class="note" style="margin:0 0 12px">${esc(ident)}</p>` : ""}
    <div class="grid">
      ${kv("Device", st?.device_status ?? "—")}
      ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
      ${kv("Squelch", st ? (st.dsp.squelch_open ? "open" : "closed") : "—")}
      ${
        r.mode === "ham" && Number(r.mode_params.ctcss_hz ?? 0) > 0
          ? kv(
              "Tone",
              `${Number(r.mode_params.ctcss_hz).toFixed(1)} Hz ${st?.dsp.ctcss_tone_hz != null ? "✓" : "…"}`,
            )
          : r.mode === "ham" && Number(r.mode_params.dcs_code ?? 0) > 0
            ? kv(
                "DCS",
                `D${String(Number(r.mode_params.dcs_code)).padStart(3, "0")}${Number(r.mode_params.dcs_invert ?? 0) ? "I" : "N"} ${st?.dsp.dcs_code != null ? "✓" : "…"}`,
              )
            : ""
      }
      ${kv("Audio", `${r.audio.sample_rate_hz / 1000} kHz · ${r.audio.channels === 1 ? "mono" : `${r.audio.channels}ch`} · ${r.audio.opus_bitrate_bps / 1000} kbps`)}
    </div>
    <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
      <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
        ${r.running ? "Stop radio" : "Start radio"}
      </button>
      ${
        canSeek && !scanning
          ? `<button id="seek-down" class="secondary" ${state.seeking ? "disabled" : ""}>◀◀ Seek</button>
             <button id="seek-up" class="secondary" ${state.seeking ? "disabled" : ""}>Seek ▶▶</button>`
          : ""
      }
      ${
        canScan && r.running
          ? `<button id="scan-toggle" class="${scanning ? "" : "secondary"}">${scanning ? "⏹ Stop scan" : "⏱ Scan"}</button>`
          : ""
      }
      <button id="radio-options" class="secondary">⚙ Radio options</button>
      ${state.seeking ? `<span class="note" style="align-self:center">seeking…</span>` : ""}
    </div>
    ${
      (r.mode === "frs" || r.mode === "ham") &&
      r.running &&
      state.server?.capabilities.includes("ptt")
        ? pttInner(st)
        : ""
    }`;
}

/** Push-to-talk block for the FRS "Now playing" card — only rendered when
 *  the server actually advertises `ptt` (device tx-capable *and*
 *  `--enable-tx`; see docs/architecture.md). Live mic audio when granted;
 *  silence otherwise (see `acquireMicIfNeeded`) — either way the button
 *  behaves the same, so it's always shown once `ptt` is available. */
function pttInner(st: RadioStatus | null): string {
  const keyed = st?.dsp.tx_keyed ?? false;
  const hasMic = !!micStream;
  const r = state.radio!;
  const rp =
    r.mode === "ham" && state.activeRepeater?.output_hz === r.frequency_hz
      ? state.activeRepeater
      : null;
  const txHz = rp ? rp.output_hz + rp.offset_hz : r.frequency_hz;
  const mp = r.mode_params ?? {};
  const encTone = rp?.tone_hz || Number(mp.ctcss_hz ?? 0);
  const encDcs = !rp && Number(mp.ctcss_hz ?? 0) === 0 ? Number(mp.dcs_code ?? 0) : 0;
  const txLine =
    r.mode === "ham"
      ? `TX ${(txHz / 1e6).toFixed(4)} MHz${rp && rp.offset_hz ? ` (repeater input, ${offsetLabel(rp.offset_hz)})` : " (simplex)"}` +
        (encTone ? ` · encoding CTCSS ${encTone.toFixed(1)} Hz` : "") +
        (encDcs
          ? ` · encoding DCS D${String(encDcs).padStart(3, "0")}${Number(mp.dcs_invert ?? 0) ? "I" : "N"}`
          : "")
      : null;
  return `
    <div style="margin-top:14px; padding-top:14px; border-top:1px solid var(--line)">
      <button id="ptt-button" class="ptt-btn${state.pttHeld ? " ptt-active" : ""}">
        ${state.pttHeld ? "🔴 Transmitting — release to stop" : "🎙️ Hold to talk"}
      </button>
      ${txLine ? `<p class="note" style="margin:8px 0 0"><strong>${esc(txLine)}</strong></p>` : ""}
      <p class="note" style="margin:8px 0 0">
        ${
          keyed
            ? "Server confirms: on the air."
            : hasMic
              ? "Mic ready — hold the button (or the spacebar) to transmit."
              : "No mic granted — holding will key up but transmit silence."
        }
        ${
          r.mode === "ham"
            ? " Amateur transmit under KZ4AZ (Part 97); homebrew gear is permitted, you are the control operator."
            : ""
        }
      </p>
      ${txLogHtml()}
    </div>`;
}

function txLogHtml(): string {
  if (state.txLog.length === 0) return "";
  const rows = state.txLog
    .map((e) => {
      const t = new Date(e.keyed_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
      const dur = e.duration_ms != null ? `${(e.duration_ms / 1000).toFixed(1)}s` : "open";
      const bits = [
        t,
        e.mode,
        `${(e.tx_frequency_hz / 1e6).toFixed(4)}${e.offset_hz ? ` ${offsetLabel(e.offset_hz)}` : ""}`,
        e.tone_hz ? `${e.tone_hz.toFixed(1)} Hz` : null,
        dur,
      ].filter(Boolean);
      return `<li>${esc(bits.join(" · "))}</li>`;
    })
    .join("");
  return `<details class="tx-log"><summary>Transmit log (${state.txLog.length})</summary><ul>${rows}</ul></details>`;
}

function audioInner(): string {
  const s = state.audioState;
  const playing = s === "playing";
  const busy = s === "connecting";
  const dotClass = playing ? "ok live" : s === "failed" ? "bad" : busy ? "warn live" : "";
  const label =
    s === "playing"
      ? "playing"
      : s === "connecting"
        ? "connecting…"
        : s === "failed"
          ? `failed${state.audioDetail ? ` — ${state.audioDetail}` : ""}`
          : "stopped";
  const stats = state.audioStats;
  const rec = state.status?.recording;
  const canRecord = ["nbfm", "wbfm", "am", "frs", "ham", "debug_tone"].includes(
    state.radio?.mode ?? "",
  );
  const lr = rec?.last;
  return `
    <h2>Received audio</h2>
    <div class="mode-line">
      <span class="badge"><span class="dot ${dotClass}"></span>${esc(label)}</span>
      ${rec?.active ? `<span class="badge"><span class="dot bad live"></span>recording</span>` : ""}
    </div>
    <div style="display:flex; gap:12px; align-items:center; flex-wrap:wrap">
      <button id="audio-toggle" class="${playing || busy ? "secondary" : ""}" ${busy ? "disabled" : ""}>
        ${playing || busy ? "Stop" : "▶ Play"}
      </button>
      <label class="note" style="display:flex; gap:6px; align-items:center">
        <input id="audio-mute" type="checkbox" ${state.audio?.muted ? "checked" : ""} /> mute
      </label>
      ${
        canRecord
          ? `<button id="audio-rec" class="secondary">${rec?.active ? "⏹ Stop recording" : "⏺ Record"}</button>`
          : ""
      }
    </div>
    ${
      lr && !rec?.active
        ? `<p class="note" style="margin:8px 0 0">saved <b>${esc(lr.filename)}</b>
           (${lr.secs.toFixed(1)}s, ${(lr.bytes / 1e6).toFixed(1)} MB) —
           <a href="${state.client?.audioRecordingUrl() ?? "#"}" download>download</a></p>`
        : ""
    }
    ${
      stats
        ? `<div class="grid" style="margin-top:14px">
             ${kv("Server state", esc(stats.state))}
             ${kv("ICE", esc(stats.ice_state))}
             ${kv("Packets", String(stats.packets_sent))}
           </div>`
        : `<p class="note" style="margin-top:10px">Opus over WebRTC — press Play (a user gesture starts audio).</p>`
    }`;
}

function telemetryInner(): string {
  const st = state.status;
  return `
    <h2>Telemetry</h2>
    <div class="grid">
      ${kv("Clients", String(st?.clients ?? "—"))}
      ${kv("Frames sent", String(st?.audio.frames_sent ?? "—"))}
      ${kv("SNR", st?.dsp.snr_db != null ? `${st.dsp.snr_db.toFixed(1)} dB` : "—")}
      ${kv("Pipeline latency", st?.dsp.pipeline_latency_ms != null ? `${st.dsp.pipeline_latency_ms.toFixed(0)} ms` : "—")}
      ${kv("Overruns", String(st?.dsp.sample_overruns ?? "—"))}
    </div>`;
}

function serverHtml(): string {
  const s = state.server!;
  const dev = state.device ?? s.selected_device;
  const sdrs = sdrDevices();
  const curId = state.device?.id;
  const devPicker =
    sdrs.length > 1
      ? `<div class="connect-row" style="margin-top:12px">
           <label class="note" for="device-pick" style="align-self:center">Radio</label>
           <select id="device-pick" ${state.switching ? "disabled" : ""}>
             ${sdrs
               .map(
                 (d) =>
                   `<option value="${esc(d.id)}"${
                     d.id === curId || d.label === state.device?.label ? " selected" : ""
                   }>${esc(d.label)}</option>`,
               )
               .join("")}
           </select>
           ${state.switching ? `<span class="note" style="align-self:center">switching…</span>` : ""}
         </div>
         <p class="note" style="margin:6px 0 0">Switching stops the receiver; adjust gain in ⚙ Radio options after.</p>`
      : "";
  return `
    <section class="card">
      <h2>Server</h2>
      <div class="grid">
        ${kv("Host", esc(s.hostname))}
        ${kv("Version", `${esc(s.version)} · proto ${s.protocol_version}`)}
        ${kv("Ports", `c2 ${s.ports.c2} · audio ${s.ports.audio_out}`)}
        ${kv("Device", dev ? `${esc(dev.label)} · ${esc(dev.driver)}` : "none")}
        ${kv("Session", state.session!.session_id.slice(0, 8))}
      </div>
      ${devPicker}
      <div class="chips" style="margin-top:12px">
        ${s.capabilities.map((c) => `<span class="chip">${esc(c)}</span>`).join("")}
      </div>
    </section>`;
}

function connStatusHtml(): string {
  switch (state.phase) {
    case "disconnected":
      return `<span class="badge"><span class="dot"></span>Disconnected</span>`;
    case "connecting":
      return `<span class="badge"><span class="dot warn live"></span>Connecting to ${esc(state.base)}…</span>`;
    case "connected":
      return `<span class="badge"><span class="dot ok live"></span>Connected to ${esc(state.base)}</span>`;
    case "error":
      return `<span class="badge"><span class="dot bad"></span><span class="err">${esc(state.error ?? "error")}</span></span>`;
  }
}

function kv(k: string, v: string): string {
  return `<div class="kv"><div class="k">${esc(k)}</div><div class="v">${v}</div></div>`;
}

function esc(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

installDelegates();
render();
