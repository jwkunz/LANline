// Thin typed REST client for the SDR C2 server.

import type {
  ApiErrorBody,
  CreateSessionResponse,
  HealthResponse,
  ModeInfo,
  RadioConfig,
  RadioStatus,
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
    opts: { body?: unknown; auth?: boolean } = {},
  ): Promise<T> {
    const headers: Record<string, string> = {};
    if (opts.body !== undefined) headers["content-type"] = "application/json";
    if (opts.auth && this.token) headers["authorization"] = `Bearer ${this.token}`;

    let res: Response;
    try {
      res = await fetch(this.base + path, {
        method,
        headers,
        body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
      });
    } catch (e) {
      throw new ApiError(
        0,
        "network",
        `cannot reach ${this.base} (${(e as Error).message})`,
      );
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

  health = () => this.request<HealthResponse>("GET", "/health");
  server = () => this.request<ServerInfo>("GET", "/api/v1/server");
  modes = () => this.request<ModeInfo[]>("GET", "/api/v1/modes");
  radio = () => this.request<RadioConfig>("GET", "/api/v1/radio");
  radioStatus = () => this.request<RadioStatus>("GET", "/api/v1/radio/status");

  createSession = (client: {
    name: string;
    user_agent: string;
    capabilities: string[];
  }) =>
    this.request<CreateSessionResponse>("POST", "/api/v1/sessions", {
      body: { client },
    });

  heartbeat = (id: string) =>
    this.request<{ expires_in_s: number }>(
      "POST",
      `/api/v1/sessions/${id}/heartbeat`,
      { auth: true },
    );

  deleteSession = (id: string) =>
    this.request<null>("DELETE", `/api/v1/sessions/${id}`, { auth: true });

  startRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/start", { auth: true });

  stopRadio = () =>
    this.request<RadioConfig>("POST", "/api/v1/radio/stop", { auth: true });
}
