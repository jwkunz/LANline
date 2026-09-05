// Thin typed REST client for the LANline server.

import type {
  AnalysisStatus,
  ApiErrorBody,
  AudioStateResponse,
  CreateSessionResponse,
  DeviceInfo,
  HealthResponse,
  ModeInfo,
  RadioConfig,
  RadioStatus,
  SdpMessage,
  ServerInfo,
} from "./types";
import { parseAptImage, type AptImage, type AptStatus } from "./apt";

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
    public details?: unknown,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Turn a user-entered host ("192.168.1.5:41785", "http://host:8080") into a
 *  base URL with no trailing slash. A bare host with no scheme inherits the
 *  page's own scheme, so opening the client over https keeps typed hosts on
 *  https too (the server serves one or the other, not both — see `--tls`). */
export function normalizeBase(host: string): string {
  let h = host.trim().replace(/\/+$/, "");
  if (!/^https?:\/\//i.test(h)) {
    const scheme =
      typeof location !== "undefined" && location.protocol === "https:" ? "https" : "http";
    h = `${scheme}://${h}`;
  }
  return h;
}

export class Client {
  private token: string | null = null;

  constructor(public readonly base: string) {}

  setToken(token: string | null): void {
    this.token = token;
  }

  private async request<T>(
    method: string,
    path: string,
    opts: { body?: unknown; auth?: boolean; signal?: AbortSignal; timeoutMs?: number } = {},
  ): Promise<T> {
    const headers: Record<string, string> = {};
    if (opts.body !== undefined) headers["content-type"] = "application/json";
    if (opts.auth && this.token) headers["authorization"] = `Bearer ${this.token}`;

    // A WebView request can wedge indefinitely (e.g. the network stack gets
    // suspended mid-request during an app-launch activity transition); a bare
    // fetch() with no signal then hangs forever with nothing to catch it. Race
    // it against a timeout, and let the caller cancel too (see main.ts).
    const controller = new AbortController();
    const timeoutMs = opts.timeoutMs ?? 10_000;
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);
    const onExternalAbort = () => controller.abort();
    opts.signal?.addEventListener("abort", onExternalAbort);

    let res: Response;
    try {
      res = await fetch(this.base + path, {
        method,
        headers,
        body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
        signal: controller.signal,
      });
    } catch (e) {
      if (timedOut) {
        throw new ApiError(0, "timeout", `${this.base}${path} timed out after ${timeoutMs}ms`);
      }
      if (opts.signal?.aborted) {
        throw new ApiError(0, "cancelled", `${this.base}${path} cancelled`);
      }
      throw new ApiError(
        0,
        "network",
        `cannot reach ${this.base} (${(e as Error).message})`,
      );
    } finally {
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onExternalAbort);
    }

    const text = await res.text();
    const data = text ? JSON.parse(text) : null;

    if (!res.ok) {
      const err = (data as ApiErrorBody | null)?.error;
      throw new ApiError(
        res.status,
        err?.code ?? "http_error",
        err?.message ?? `${res.status} ${res.statusText}`,
        err?.details,
      );
    }
    return data as T;
  }

  health = (signal?: AbortSignal) => this.request<HealthResponse>("GET", "/health", { signal });
  server = (signal?: AbortSignal) =>
    this.request<ServerInfo>("GET", "/api/v1/server", { signal });
  device = (signal?: AbortSignal) =>
    this.request<DeviceInfo>("GET", "/api/v1/device", { signal });
  modes = (signal?: AbortSignal) => this.request<ModeInfo[]>("GET", "/api/v1/modes", { signal });
  radio = (signal?: AbortSignal) => this.request<RadioConfig>("GET", "/api/v1/radio", { signal });
  radioStatus = (signal?: AbortSignal) =>
    this.request<RadioStatus>("GET", "/api/v1/radio/status", { signal });
  adsbAircraft = () => this.request<import("./adsb").AdsbSnapshot>("GET", "/api/v1/adsb/aircraft");
  aisVessels = () => this.request<import("./ais").AisSnapshot>("GET", "/api/v1/ais/vessels");
  aptStatus = () => this.request<AptStatus>("GET", "/api/v1/apt/status");

  /** Binary raster, not JSON — fetched and parsed separately from `request()`.
   *  Can grow to a couple MB once a pass has been running a while, so it
   *  gets its own (longer) timeout rather than the default 10s. */
  async aptImage(signal?: AbortSignal, timeoutMs = 10_000): Promise<AptImage> {
    const controller = new AbortController();
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);
    const onExternalAbort = () => controller.abort();
    signal?.addEventListener("abort", onExternalAbort);

    let res: Response;
    try {
      res = await fetch(this.base + "/api/v1/apt/image", { signal: controller.signal });
    } catch (e) {
      if (timedOut) {
        throw new ApiError(0, "timeout", `apt/image timed out after ${timeoutMs}ms`);
      }
      if (signal?.aborted) {
        throw new ApiError(0, "cancelled", "apt/image cancelled");
      }
      throw new ApiError(0, "network", `cannot reach ${this.base} (${(e as Error).message})`);
    } finally {
      clearTimeout(timer);
      signal?.removeEventListener("abort", onExternalAbort);
    }
    if (!res.ok) {
      throw new ApiError(res.status, "http_error", `${res.status} ${res.statusText}`);
    }
    return parseAptImage(await res.arrayBuffer());
  }

  createSession = (
    client: { name: string; user_agent: string; capabilities: string[] },
    signal?: AbortSignal,
  ) =>
    this.request<CreateSessionResponse>("POST", "/api/v1/sessions", {
      body: { client },
      signal,
    });

  heartbeat = (id: string) =>
    this.request<{ expires_in_s: number }>(
      "POST",
      `/api/v1/sessions/${id}/heartbeat`,
      { auth: true },
    );

  deleteSession = (id: string) =>
    this.request<null>("DELETE", `/api/v1/sessions/${id}`, { auth: true });

  patchRadio = (patch: Record<string, unknown>) =>
    this.request<RadioConfig>("PATCH", "/api/v1/radio", { auth: true, body: patch });

  // --- Receiver Analysis -------------------------------------------
  analysisStatus = (signal?: AbortSignal) =>
    this.request<AnalysisStatus>("GET", "/api/v1/analysis/status", { signal });

  /** Binary spectrum/waterfall frame (`LWF1` — see analysis.ts). Bare fetch,
   *  no retry: a dropped frame just means one skipped waterfall row. */
  async analysisSpectrum(sinceSeq: number, maxRows: number): Promise<ArrayBuffer> {
    const res = await fetch(
      `${this.base}/api/v1/analysis/spectrum?since=${sinceSeq}&max_rows=${maxRows}`,
    );
    if (!res.ok) throw new ApiError(res.status, "http_error", `${res.status} ${res.statusText}`);
    return res.arrayBuffer();
  }

  analysisRecord = (action: "start" | "stop", maxSecs?: number) =>
    this.request<Record<string, unknown>>("POST", "/api/v1/analysis/record", {
      auth: true,
      body: { action, ...(maxSecs != null ? { max_secs: maxSecs } : {}) },
    });

  analysisRecordingUrl = () => `${this.base}/api/v1/analysis/recording`;

  startRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/start", { auth: true });

  stopRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/stop", { auth: true });

  /** Push-to-talk: begins a transmission (currently a synthesized test
   *  tone only, not live mic audio — see docs/architecture.md). Server-side
   *  gated behind `--enable-tx` + `frs` mode + a tx-capable device, and
   *  auto-unkeys after a hard server-side cap regardless of `unkeyTx`. */
  keyTx = (gainDb?: number) =>
    this.request<{ keyed: boolean; gain_db: number }>("POST", "/api/v1/radio/tx/key", {
      auth: true,
      body: gainDb != null ? { gain_db: gainDb } : {},
    });

  unkeyTx = () =>
    this.request<{ keyed: boolean }>("POST", "/api/v1/radio/tx/unkey", { auth: true });

  audioOffer = (id: string, sdp: string) =>
    this.request<SdpMessage>("POST", `/api/v1/sessions/${id}/audio/offer`, {
      auth: true,
      body: { sdp, type: "offer" },
    });

  audioClose = (id: string) =>
    this.request<null>("DELETE", `/api/v1/sessions/${id}/audio`, { auth: true });

  audioState = (id: string) =>
    this.request<AudioStateResponse>("GET", `/api/v1/sessions/${id}/audio`, {
      auth: true,
    });
}
