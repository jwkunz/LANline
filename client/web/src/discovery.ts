// LAN discovery. Browsers cannot receive the UDP beacon, so this only does
// anything inside the Android wrapper, which injects `window.LanlineNative`.

export interface DiscoveredServer {
  server_id: string;
  hostname: string;
  c2_base_url: string;
  ports: { c2: number; audio_out: number; audio_in: number };
  device: string | null;
  age_ms: number;
}

declare global {
  interface Window {
    LanlineNative?: {
      platform(): string;
      discoveredServers(): string;
    };
  }
}

/** Returns a poll function when running inside a host that provides discovery,
 *  otherwise `null`. */
export function nativeDiscovery(): (() => DiscoveredServer[]) | null {
  const n = window.LanlineNative;
  if (!n || typeof n.discoveredServers !== "function") return null;
  return () => {
    try {
      const list = JSON.parse(n.discoveredServers()) as DiscoveredServer[];
      return Array.isArray(list) ? list : [];
    } catch {
      return [];
    }
  };
}

/** Host string (`host:port`) to type into the connect field for a server. */
export function serverHost(s: DiscoveredServer): string {
  if (s.c2_base_url) return s.c2_base_url;
  return `${s.hostname}:${s.ports.c2}`;
}
