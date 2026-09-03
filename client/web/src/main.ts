import "./style.css";
import { ApiError, Client, normalizeBase } from "./api";
import type {
  CreateSessionResponse,
  ModeInfo,
  RadioConfig,
  RadioStatus,
  ServerInfo,
} from "./types";

const HOST_KEY = "sdrc2.host";

type Phase = "disconnected" | "connecting" | "connected" | "error";

interface State {
  phase: Phase;
  base: string;
  error: string | null;
  client: Client | null;
  session: CreateSessionResponse | null;
  server: ServerInfo | null;
  radio: RadioConfig | null;
  status: RadioStatus | null;
  modeNames: Record<string, string>;
  log: string[];
}

const state: State = {
  phase: "disconnected",
  base: "",
  error: null,
  client: null,
  session: null,
  server: null,
  radio: null,
  status: null,
  modeNames: {},
  log: [],
};

let heartbeatTimer: number | undefined;
let pollTimer: number | undefined;
let pollFails = 0;

// --- shell -----------------------------------------------------------------

const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `
  <h1>SDR&nbsp;C2 <span class="sub">web client</span></h1>
  <p class="tagline">Discover a radio server on the LAN and watch the receive session.</p>
  <section class="card">
    <h2>Connection</h2>
    <div class="connect-row">
      <input id="host" type="text" spellcheck="false" autocapitalize="off"
             placeholder="server host, e.g. 192.168.1.50:41785" />
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

hostInput.value = localStorage.getItem(HOST_KEY) ?? "";
actionBtn.addEventListener("click", onAction);
hostInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") onAction();
});

// --- actions -------------------------------------------------------------

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

  const client = new Client(base);
  try {
    await client.health();
    const server = await client.server();
    const modes = await client.modes().catch(() => [] as ModeInfo[]);
    const session = await client.createSession({
      name: "web",
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
    state.modeNames = Object.fromEntries(modes.map((m) => [m.id, m.name]));
    setState({ phase: "connected", server, radio, status, error: null });
    logLine(
      `connected to ${server.hostname} · server ${server.server_id.slice(0, 8)} · session ${session.session_id.slice(0, 8)}`,
    );
    startTimers();
  } catch (e) {
    const err = e as ApiError;
    state.client = null;
    state.session = null;
    setState({ phase: "error", error: `${err.code}: ${err.message}` });
    logLine(`connect failed — ${err.message}`);
  }
}

async function disconnect(): Promise<void> {
  stopTimers();
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
    radio: null,
    status: null,
    error: null,
  });
  logLine("disconnected");
}

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
    setState({ server, radio, status });
  } catch (e) {
    pollFails += 1;
    if (pollFails >= 3) loopFailed(e as ApiError, "poll");
  }
}

function loopFailed(err: ApiError, where: string): void {
  stopTimers();
  state.client = null;
  state.session = null;
  setState({ phase: "error", error: `lost connection (${where}: ${err.message})` });
  logLine(`connection lost — ${where}: ${err.message}`);
}

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
  panels.innerHTML = connected ? panelsHtml() : idlePanelsHtml();

  const toggle = panels.querySelector<HTMLButtonElement>("#radio-toggle");
  if (toggle) toggle.addEventListener("click", () => void toggleRadio());
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

function idlePanelsHtml(): string {
  return `
    <section class="card">
      <h2>Session</h2>
      <p class="note">No active session. Enter the server host shown in the server log
      (or from the discovery beacon) and connect.</p>
    </section>
    ${logCardHtml()}
  `;
}

function panelsHtml(): string {
  const s = state.server!;
  const r = state.radio!;
  const st = state.status;
  const dev = s.selected_device;
  const modeName = state.modeNames[r.mode] ?? r.mode;
  const freqMhz = (r.frequency_hz / 1e6).toFixed(4);
  const runDot = r.running ? "ok live" : "";
  const dspRssi =
    st?.dsp.rssi_dbfs != null ? `${st.dsp.rssi_dbfs.toFixed(1)} dBFS` : "—";
  const dspSnr =
    st?.dsp.snr_db != null ? `${st.dsp.snr_db.toFixed(1)} dB` : "—";
  const dspLat =
    st?.dsp.pipeline_latency_ms != null
      ? `${st.dsp.pipeline_latency_ms.toFixed(0)} ms`
      : "—";

  return `
    <section class="card">
      <h2>Radio</h2>
      <div class="mode-line">
        <span class="mode">${esc(modeName)}</span>
        <span class="freq">${freqMhz} MHz</span>
        <span class="badge"><span class="dot ${runDot}"></span>${r.running ? "running" : "stopped"}</span>
      </div>
      <div class="grid">
        ${kv("Device status", st?.device_status ?? "—")}
        ${kv("Audio", `${r.audio.sample_rate_hz / 1000} kHz · ${r.audio.channels === 1 ? "mono" : `${r.audio.channels} ch`} · Opus ${r.audio.opus_bitrate_bps / 1000} kbps`)}
        ${kv("Squelch", st ? (st.dsp.squelch_open ? "open" : "closed") : "—")}
      </div>
      <div style="margin-top:14px">
        <button id="radio-toggle" class="${r.running ? "secondary" : ""}">
          ${r.running ? "Stop radio" : "Start radio"}
        </button>
      </div>
    </section>

    <section class="card">
      <h2>Received audio</h2>
      <p class="note">Streaming (WebRTC / Opus) arrives in phase 1b. This build
      exercises discovery, the REST session and telemetry only.</p>
    </section>

    <section class="card">
      <h2>Telemetry</h2>
      <div class="grid">
        ${kv("Clients", String(st?.clients ?? "—"))}
        ${kv("Frames sent", String(st?.audio.frames_sent ?? "—"))}
        ${kv("RSSI", dspRssi)}
        ${kv("SNR", dspSnr)}
        ${kv("Pipeline latency", dspLat)}
        ${kv("Overruns", String(st?.dsp.sample_overruns ?? "—"))}
      </div>
    </section>

    <section class="card">
      <h2>Server</h2>
      <div class="grid">
        ${kv("Host", esc(s.hostname))}
        ${kv("Version", `${esc(s.version)} · proto ${s.protocol_version}`)}
        ${kv("Server ID", s.server_id.slice(0, 8))}
        ${kv("C2 port", String(s.ports.c2))}
        ${kv("Audio-out port", String(s.ports.audio_out))}
        ${kv("Audio-in port", `${s.ports.audio_in} (reserved)`)}
        ${kv("Device", dev ? `${esc(dev.label)} · ${esc(dev.driver)}` : "none selected")}
      </div>
      <div class="chips" style="margin-top:12px">
        ${s.capabilities.map((c) => `<span class="chip">${esc(c)}</span>`).join("")}
      </div>
    </section>

    <section class="card">
      <h2>Session</h2>
      <div class="grid">
        ${kv("Session ID", state.session!.session_id.slice(0, 8))}
        ${kv("Heartbeat", `every ${state.session!.heartbeat_interval_s}s`)}
      </div>
    </section>

    ${logCardHtml()}
  `;
}

function logCardHtml(): string {
  return `
    <section class="card">
      <h2>Event log</h2>
      <div id="log">${state.log.map(esc).join("\n") || "—"}</div>
    </section>
  `;
}

function kv(k: string, v: string): string {
  return `<div class="kv"><div class="k">${esc(k)}</div><div class="v">${v}</div></div>`;
}

function esc(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[c]!,
  );
}

render();
