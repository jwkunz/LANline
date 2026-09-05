import "./style.css";
import { ApiError, Client, normalizeBase } from "./api";
import { openRadioOptions, radioOptionsOpen } from "./radio-options";
import { AnalysisView } from "./analysis";
import { AudioSession, type AudioState } from "./audio";
import { nativeDiscovery, serverHost } from "./discovery";
import { parseLatLon, type Located } from "./geo";
import { altLabel, offsetNm, type AdsbSnapshot } from "./adsb";
import { type AisSnapshot } from "./ais";
import { loadNwrStations, nearestNwr, type NwrStation } from "./nwr";
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
import type {
  AudioStateResponse,
  CreateSessionResponse,
  DeviceInfo,
  ModeInfo,
  RadioConfig,
  RadioStatus,
  ServerInfo,
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

interface State {
  phase: Phase;
  base: string;
  error: string | null;
  client: Client | null;
  session: CreateSessionResponse | null;
  server: ServerInfo | null;
  device: DeviceInfo | null;
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
  seeking: boolean;
  pttHeld: boolean;
  adsb: AdsbSnapshot | null;
  ais: AisSnapshot | null;
  apt: AptStatus | null;
  scopeSel: string | null;
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
  seeking: false,
  pttHeld: false,
  adsb: null,
  ais: null,
  apt: null,
  scopeSel: null,
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
  <h1>LANline <span class="sub">SDR on your LAN</span></h1>
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
  if (m === "adsb" || m === "ais" || m === "apt") return; // data-only modes, no audio
  try {
    if (state.audioState === "idle") await startAudio();
  } catch (e) {
    logLine(`auto play failed — ${(e as Error).message}`);
  }
}

/** Mic permission is requested only for `frs` (the one PTT-capable mode),
 *  and only when the server actually offers `ptt` — no point prompting for
 *  a mic that has nowhere to go. Failure (denied, no device) degrades to
 *  receive-only, not a hard error: normal FRS listening still works, PTT
 *  just won't have real audio behind it (silence — see `TxAudioSource`). */
async function acquireMicIfNeeded(): Promise<MediaStream | null> {
  if (state.radio?.mode !== "frs" || !state.server?.capabilities.includes("ptt")) return null;
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

function toggleMute(): void {
  if (state.audio) {
    state.audio.setMuted(!state.audio.muted);
    render();
  }
}

/** Largest device-supported sample rate ≤ want (else the smallest available). */
function pickSampleRate(wantHz: number): number | null {
  const ranges = state.device?.rx.sample_rate_ranges_hz ?? [];
  const cands = ranges.flatMap((r) => [r.min, r.max]).filter((v) => v > 0);
  if (cands.length === 0) return null;
  const under = cands.filter((v) => v <= wantHz + 1);
  return under.length ? Math.max(...under) : Math.min(...cands);
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
    setState({ radio, adsb: null, ais: null, apt: null, scopeSel: null });
    logLine(`mode → ${meta?.label ?? id}`);
    void refreshStations();
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
    setState({ radio });
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
    const r = await state.client.keyTx();
    logLine(
      micStream
        ? `PTT keyed — mic live, ${r.gain_db} dB`
        : `PTT keyed — no mic granted, transmitting silence (${r.gain_db} dB)`,
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

async function refreshStations(): Promise<void> {
  const loc = state.loc;
  const mode = state.radio?.mode;
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
    const dataMode = radio.mode === "adsb" || radio.mode === "ais" || radio.mode === "apt";
    let audioStats = state.audioStats;
    if (!dataMode && state.session && state.audio && state.audioState !== "idle") {
      audioStats = await state.client
        .audioState(state.session.session_id)
        .catch(() => state.audioStats);
    }
    let adsb = state.adsb;
    let ais = state.ais;
    let apt = state.apt;
    if (radio.mode === "adsb") {
      adsb = await state.client.adsbAircraft().catch(() => state.adsb);
    } else if (radio.mode === "ais") {
      ais = await state.client.aisVessels().catch(() => state.ais);
    } else if (radio.mode === "apt") {
      apt = await state.client.aptStatus().catch(() => state.apt);
      // The raster can grow to a couple MB; refetch it far less often than
      // the 1s status/telemetry cadence.
      if (Date.now() - aptImageFetchedAt > 3_000) {
        aptImageFetchedAt = Date.now();
        void refreshAptImage();
      }
    }
    setState({ server, radio, status, audioStats, adsb, ais, apt });
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
    state.adsb || state.ais ? 1 : 0,
    state.apt ? 1 : 0,
    state.scopeRangeNm,
  ].join("|");
}

function render(): void {
  const connected = state.phase === "connected";
  const connecting = state.phase === "connecting";

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

  if (state.radio.mode === "adsb" || state.radio.mode === "ais") {
    setHTML("#scope-list", scopeListInner());
    drawScope();
  } else if (state.radio.mode === "apt") {
    setHTML("#apt-status", aptStatusInner());
    setHTML("#apt-sat-rows", aptSatRowsInner());
    drawAptImage();
  }

  const freq = state.radio.frequency_hz;
  panels.querySelectorAll<HTMLButtonElement>(".station").forEach((b) => {
    b.classList.toggle("tuned", Number(b.dataset.hz) === freq);
  });

  // Keep the manual-tune dial's number in sync as +/- and Seek move the
  // frequency — structKey() ignores frequency_hz, so the panel isn't
  // rebuilt. Don't stomp a value the user is mid-edit on.
  syncAnalysisView();

  const dial = q<HTMLInputElement>("#fm-freq, #am-freq, #apt-freq");
  if (dial && dial !== document.activeElement) {
    const v =
      dial.id === "am-freq"
        ? (freq / 1e3).toFixed(0)
        : dial.id === "apt-freq"
          ? (freq / 1e6).toFixed(4)
          : (freq / 1e6).toFixed(1);
    if (dial.value !== v) dial.value = v;
  }
}

// --- delegated events (wired once) ----------------------------------

function installDelegates(): void {
  panels.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const hit = (sel: string) => t.closest(sel);

    const mode = t.closest<HTMLButtonElement>(".mode-btn");
    if (mode) return void switchMode(mode.dataset.mode!);
    if (hit("#radio-toggle")) return void toggleRadio();
    if (hit("#radio-options")) return void openRadioOptionsPanel();
    if (hit("#audio-toggle")) return void toggleAudio();
    if (hit("#loc-find")) return findFromInput();
    if (hit("#loc-me")) return useMyLocation();
    if (hit("#seek-down")) return void seek(-1);
    if (hit("#seek-up")) return void seek(1);
    if (hit("#scope-recenter")) return setState({ scopeSel: null });
    if (t.id === "scope") return onScopeClick(e as MouseEvent);

    const acRow = t.closest<HTMLButtonElement>(".ac-row");
    if (acRow) {
      const cid = acRow.dataset.cid ?? null;
      return setState({ scopeSel: cid === state.scopeSel ? null : cid });
    }
    if (hit("#fm-down")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, -1));
    if (hit("#fm-up")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, 1));
    if (hit("#fm-go")) return fmManualGo();
    if (hit("#am-down")) return void tuneFrequency(stepAm(state.radio!.frequency_hz, -1));
    if (hit("#am-up")) return void tuneFrequency(stepAm(state.radio!.frequency_hz, 1));
    if (hit("#am-go")) return amManualGo();
    if (hit("#apt-go")) return aptManualGo();

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
  // open, and unless FRS PTT is actually available.
  const pttHotkeyOk = (t: EventTarget | null): boolean => {
    if (
      state.radio?.mode !== "frs" ||
      !state.radio.running ||
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
            return `<button class="mode-btn${id === cur ? " on" : ""}" data-mode="${esc(id)}" ${state.switching ? "disabled" : ""}>
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
      return frsWizardHtml();
    case "adsb":
    case "ais":
      return scopeWizardHtml();
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
  kind: "air" | "sea";
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
  const rows = s.contacts
    .map(
      (c) =>
        `<button class="ac-row${c.id === sel ? " tuned" : ""}${c.lat == null ? " noloc" : ""}" data-cid="${esc(c.id)}">
        <span class="s-call">${esc(c.label)}</span>
        <span class="s-site">${esc(c.sub)}</span>
      </button>`,
    )
    .join("");
  return `<div class="stations ac-table">${rows}</div>`;
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
    ctx.fillStyle = selected ? accent : c.kind === "sea" ? "#7fc8ff" : "#8fe0a0";
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
  setState({ scopeSel: best ? (best.id === state.scopeSel ? null : best.id) : null });
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
  const canSeek = r.mode === "wbfm" || r.mode === "nbfm" || r.mode === "am" || r.mode === "frs";
  return `
    <h2>Now playing</h2>
    <div class="mode-line">
      <span class="mode">${esc(meta?.label ?? r.mode)}</span>
      <span class="freq">${freqLabel(r.mode, r.frequency_hz)}</span>
      <span class="badge"><span class="dot ${r.running ? "ok live" : ""}"></span>${r.running ? "running" : "stopped"}</span>
    </div>
    ${ident ? `<p class="note" style="margin:0 0 12px">${esc(ident)}</p>` : ""}
    <div class="grid">
      ${kv("Device", st?.device_status ?? "—")}
      ${kv("Signal", st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—")}
      ${kv("Squelch", st ? (st.dsp.squelch_open ? "open" : "closed") : "—")}
      ${kv("Audio", `${r.audio.sample_rate_hz / 1000} kHz · ${r.audio.channels === 1 ? "mono" : `${r.audio.channels}ch`} · ${r.audio.opus_bitrate_bps / 1000} kbps`)}
    </div>
    <div style="margin-top:14px; display:flex; gap:8px; flex-wrap:wrap">
      <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
        ${r.running ? "Stop radio" : "Start radio"}
      </button>
      ${
        canSeek
          ? `<button id="seek-down" class="secondary" ${state.seeking ? "disabled" : ""}>◀◀ Seek</button>
             <button id="seek-up" class="secondary" ${state.seeking ? "disabled" : ""}>Seek ▶▶</button>`
          : ""
      }
      <button id="radio-options" class="secondary">⚙ Radio options</button>
      ${state.seeking ? `<span class="note" style="align-self:center">seeking…</span>` : ""}
    </div>
    ${r.mode === "frs" && r.running && state.server?.capabilities.includes("ptt") ? pttInner(st) : ""}`;
}

/** Push-to-talk block for the FRS "Now playing" card — only rendered when
 *  the server actually advertises `ptt` (device tx-capable *and*
 *  `--enable-tx`; see docs/architecture.md). Live mic audio when granted;
 *  silence otherwise (see `acquireMicIfNeeded`) — either way the button
 *  behaves the same, so it's always shown once `ptt` is available. */
function pttInner(st: RadioStatus | null): string {
  const keyed = st?.dsp.tx_keyed ?? false;
  const hasMic = !!micStream;
  return `
    <div style="margin-top:14px; padding-top:14px; border-top:1px solid var(--line)">
      <button id="ptt-button" class="ptt-btn${state.pttHeld ? " ptt-active" : ""}">
        ${state.pttHeld ? "🔴 Transmitting — release to stop" : "🎙️ Hold to talk"}
      </button>
      <p class="note" style="margin:8px 0 0">
        ${
          keyed
            ? "Server confirms: on the air."
            : hasMic
              ? "Mic ready — hold the button (or the spacebar) to transmit."
              : "No mic granted — holding will key up but transmit silence."
        }
      </p>
    </div>`;
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
  return `
    <h2>Received audio</h2>
    <div class="mode-line">
      <span class="badge"><span class="dot ${dotClass}"></span>${esc(label)}</span>
    </div>
    <div style="display:flex; gap:12px; align-items:center; flex-wrap:wrap">
      <button id="audio-toggle" class="${playing || busy ? "secondary" : ""}" ${busy ? "disabled" : ""}>
        ${playing || busy ? "Stop" : "▶ Play"}
      </button>
      <label class="note" style="display:flex; gap:6px; align-items:center">
        <input id="audio-mute" type="checkbox" ${state.audio?.muted ? "checked" : ""} /> mute
      </label>
    </div>
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
  const dev = s.selected_device;
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
