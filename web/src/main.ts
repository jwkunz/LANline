import "./style.css";
import { ApiError, Client, normalizeBase } from "./api";
import { AudioSession, type AudioState } from "./audio";
import { nativeDiscovery, serverHost } from "./discovery";
import { parseLatLon, type Located } from "./geo";
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
  fmTab: "nearby" | "manual";
  seeking: boolean;
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
  fmTab: "nearby",
  seeking: false,
  log: [],
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let heartbeatTimer: number | undefined;
let pollTimer: number | undefined;
let pollFails = 0;

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

async function connect(hostRaw: string): Promise<void> {
  if (!hostRaw.trim()) {
    setState({ phase: "error", error: "enter a server host first" });
    return;
  }
  const base = normalizeBase(hostRaw);
  localStorage.setItem(HOST_KEY, hostRaw.trim());
  setState({ phase: "connecting", base, error: null });

  console.info(`lanline: connecting ${base}`);
  const client = new Client(base);
  try {
    await client.health();
    const server = await client.server();
    const modes = await client.modes().catch(() => [] as ModeInfo[]);
    const device = await client.device().catch(() => null);
    console.info(`lanline: rest ok (${server.hostname}, ${modes.length} modes, device=${!!device})`);
    const session = await client.createSession({
      name: NATIVE ? "android" : "web",
      user_agent: navigator.userAgent,
      capabilities: ["webrtc-recv"],
    });
    client.setToken(session.token);
    const [radio, status] = await Promise.all([
      client.radio(),
      client.radioStatus(),
    ]);

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
    const err = e as ApiError;
    console.warn(`lanline: connect failed — ${err.code}: ${err.message}`);
    state.client = null;
    state.session = null;
    setState({ phase: "error", error: `${err.code}: ${err.message}` });
    logLine(`connect failed — ${err.message}`);
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
  try {
    if (state.audioState === "idle") await state.audio.start();
  } catch (e) {
    logLine(`auto play failed — ${(e as Error).message}`);
  }
}

async function disconnect(): Promise<void> {
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

async function toggleAudio(): Promise<void> {
  const audio = state.audio;
  if (!audio) return;
  if (audio.state === "playing" || audio.state === "connecting") {
    await audio.stop().catch(() => {});
    state.audioStats = null;
    render();
  } else {
    try {
      await audio.start();
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

  const patch: Record<string, unknown> = { mode: id };
  if (id !== "debug_tone") {
    const last = Number(localStorage.getItem(freqKey(id)));
    patch.frequency_hz = Number.isFinite(last) && last > 0 ? last : meta?.defaultFreqHz;
    const sr = pickSampleRate(meta?.wantSampleRateHz ?? 2_000_000);
    const tuner: Record<string, unknown> = { lo_offset_hz: meta?.loOffsetHz ?? 250_000 };
    if (sr) tuner.sample_rate_hz = sr;
    patch.tuner = tuner;
  }

  try {
    let radio = await state.client.patchRadio(patch);
    setState({ radio });
    logLine(`mode → ${meta?.label ?? id}`);
    void refreshStations();
    if (radio.running || NATIVE) {
      if (!radio.running) radio = await state.client.startRadio();
      setState({ radio });
      if (state.audio && state.audioState === "idle") void state.audio.start();
    }
  } catch (e) {
    const err = e as ApiError;
    logLine(`mode switch failed — ${err.message}`);
  } finally {
    setState({ switching: false });
  }
}

async function tuneFrequency(hz: number, label?: string): Promise<void> {
  if (!state.client || !state.radio) return;
  const freq = Math.round(hz);
  try {
    let radio = await state.client.patchRadio({ frequency_hz: freq });
    localStorage.setItem(freqKey(radio.mode), String(freq));
    if (!radio.running) radio = await state.client.startRadio();
    setState({ radio });
    if (state.audio && state.audioState === "idle") void state.audio.start();
    const digits = radio.mode === "wbfm" ? 1 : 4;
    logLine(`tuned ${label ?? `${(freq / 1e6).toFixed(digits)} MHz`}`);
  } catch (e) {
    logLine(`tune failed — ${(e as ApiError).message}`);
  }
}

/** Scan up/down for the next occupied channel. Client-driven: step, wait, read
 *  status, stop on signal. */
async function seek(dir: 1 | -1): Promise<void> {
  if (!state.client || !state.radio || state.seeking) return;
  const mode = state.radio.mode;
  if (mode !== "wbfm" && mode !== "nbfm") return;

  // Scan the actual FM broadcast band (not the wider manual-tune range).
  const step = mode === "wbfm" ? 200_000 : 25_000;
  const lo = mode === "wbfm" ? 87_700_000 : 162_400_000;
  const hi = mode === "wbfm" ? 108_100_000 : 162_550_000;
  const rssiGate = mode === "wbfm" ? -48 : -75;
  const settleMs = mode === "wbfm" ? 220 : 200;
  const steps = Math.round((hi - lo) / step) + 1;

  setState({ seeking: true });
  logLine(`seek ${dir > 0 ? "up" : "down"}…`);
  const start = state.radio.frequency_hz;
  let f = start;
  try {
    for (let i = 0; i < steps; i++) {
      f += dir * step;
      if (f > hi) f = lo;
      if (f < lo) f = hi;
      if (f === start) break; // wrapped all the way around
      await state.client.patchRadio({ frequency_hz: f });
      await sleep(settleMs);
      const s = await state.client.radioStatus().catch(() => null);
      const rssi = s?.dsp.rssi_dbfs ?? -200;
      const open = mode === "nbfm" ? (s?.dsp.squelch_open ?? false) && rssi > rssiGate : rssi > rssiGate;
      if (open) break;
    }
    localStorage.setItem(freqKey(mode), String(f));
    const radio = await state.client.radio();
    setState({ radio });
    logLine(`seek → ${(radio.frequency_hz / 1e6).toFixed(mode === "wbfm" ? 1 : 4)} MHz`);
    void refreshStations();
  } catch (e) {
    logLine(`seek failed — ${(e as ApiError).message}`);
  } finally {
    setState({ seeking: false });
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
    let audioStats = state.audioStats;
    if (state.session && state.audio && state.audioState !== "idle") {
      audioStats = await state.client
        .audioState(state.session.session_id)
        .catch(() => state.audioStats);
    }
    setState({ server, radio, status, audioStats });
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
    state.fmTab,
    state.switching,
    state.seeking,
    state.modes.length,
    state.nwrNearby.length,
    state.fmNearby.length,
    state.loc ? 1 : 0,
    state.locNote ?? "",
    state.audioStats ? 1 : 0,
  ].join("|");
}

function render(): void {
  const connected = state.phase === "connected";
  const connecting = state.phase === "connecting";

  actionBtn.textContent = connected
    ? "Disconnect"
    : connecting
      ? "Connecting…"
      : "Connect";
  actionBtn.disabled = connecting;
  actionBtn.classList.toggle("secondary", connected);
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

/** Update the values that change every poll without touching the DOM
 *  structure (so the station list keeps its scroll position). */
function patchLive(): void {
  if (!state.radio) return;
  setHTML("#now-playing", nowPlayingInner());
  setHTML("#audio-card", audioInner());
  setHTML("#telemetry", telemetryInner());
  setHTML("#log", state.log.map(esc).join("\n") || "—");

  const freq = state.radio.frequency_hz;
  panels.querySelectorAll<HTMLButtonElement>(".station").forEach((b) => {
    b.classList.toggle("tuned", Number(b.dataset.hz) === freq);
  });
}

// --- delegated events (wired once) ----------------------------------

function installDelegates(): void {
  panels.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const hit = (sel: string) => t.closest(sel);

    const mode = t.closest<HTMLButtonElement>(".mode-btn");
    if (mode) return void switchMode(mode.dataset.mode!);
    if (hit("#radio-toggle")) return void toggleRadio();
    if (hit("#audio-toggle")) return void toggleAudio();
    if (hit("#loc-find")) return findFromInput();
    if (hit("#loc-me")) return useMyLocation();
    if (hit("#seek-down")) return void seek(-1);
    if (hit("#seek-up")) return void seek(1);
    if (hit("#fm-down")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, -1));
    if (hit("#fm-up")) return void tuneFrequency(stepFm(state.radio!.frequency_hz, 1));
    if (hit("#fm-go")) return fmManualGo();

    const tab = t.closest<HTMLButtonElement>("[data-fmtab]");
    if (tab) return setState({ fmTab: tab.dataset.fmtab as "nearby" | "manual" });

    const station = t.closest<HTMLButtonElement>(".station");
    if (station) {
      const hz = Number(station.dataset.hz);
      if (hz) void tuneFrequency(hz, station.dataset.label);
    }
  });

  panels.addEventListener("change", (e) => {
    if ((e.target as HTMLElement).id === "audio-mute") toggleMute();
  });

  panels.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key !== "Enter") return;
    const id = (e.target as HTMLElement).id;
    if (id === "loc") findFromInput();
    else if (id === "fm-freq") fmManualGo();
  });
}

function fmManualGo(): void {
  const raw = panels.querySelector<HTMLInputElement>("#fm-freq")?.value ?? "";
  const mhz = parseFloat(raw);
  if (Number.isFinite(mhz)) void tuneFrequency(snapFm(mhz * 1e6));
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
  return `
    ${modeStripHtml()}
    ${wizardHtml()}
    <section class="card" id="now-playing">${nowPlayingInner()}</section>
    <section class="card" id="audio-card">${audioInner()}</section>
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
    case "debug_tone":
      return toneWizardHtml();
    default:
      return "";
  }
}

function nwrWizardHtml(): string {
  const r = state.radio!;
  const rows = state.nwrNearby
    .map((s) => {
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
        s.freq_hz === r.frequency_hz,
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
  const tab = state.fmTab;
  return `
    <section class="card">
      <h2>FM Broadcast — tune a station</h2>
      <div class="tabs">
        <button class="tab${tab === "nearby" ? " on" : ""}" data-fmtab="nearby">Nearby</button>
        <button class="tab${tab === "manual" ? " on" : ""}" data-fmtab="manual">Manual</button>
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
  const digits = r.mode === "wbfm" ? 1 : 4;
  const ident = tunedStationLabel();
  const canSeek = r.mode === "wbfm" || r.mode === "nbfm";
  return `
    <h2>Now playing</h2>
    <div class="mode-line">
      <span class="mode">${esc(meta?.label ?? r.mode)}</span>
      <span class="freq">${(r.frequency_hz / 1e6).toFixed(digits)} MHz</span>
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
      ${state.seeking ? `<span class="note" style="align-self:center">seeking…</span>` : ""}
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
