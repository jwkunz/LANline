// Thin typed REST client for the LANline server.

import type {
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
 *  base URL with no trailing slash. */
export function normalizeBase(host: string): string {
  let h = host.trim().replace(/\/+$/, "");
  if (!/^https?:\/\//i.test(h)) h = "http://" + h;
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

  startRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/start", { auth: true });

  stopRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/stop", { auth: true });

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
