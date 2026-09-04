// Shared geo helpers for the station finders.

export function haversineMi(
  aLat: number,
  aLon: number,
  bLat: number,
  bLon: number,
): number {
  const R = 3958.7613;
  const rad = (d: number) => (d * Math.PI) / 180;
  const dLat = rad(bLat - aLat);
  const dLon = rad(bLon - aLon);
  const s =
    Math.sin(dLat / 2) ** 2 +
    Math.cos(rad(aLat)) * Math.cos(rad(bLat)) * Math.sin(dLon / 2) ** 2;
  return 2 * R * Math.asin(Math.sqrt(s));
}

export interface Located {
  lat: number;
  lon: number;
}

/** Nearest items to a point, closest first, each tagged with `distance_mi`. */
export function nearest<T extends Located>(
  lat: number,
  lon: number,
  items: readonly T[],
  limit: number,
): (T & { distance_mi: number })[] {
  return items
    .map((s) => ({ ...s, distance_mi: haversineMi(lat, lon, s.lat, s.lon) }))
    .sort((a, b) => a.distance_mi - b.distance_mi)
    .slice(0, limit);
}

/** Parse "lat, lon" / "lat lon" loosely. */
export function parseLatLon(text: string): Located | null {
  const m = text
    .trim()
    .replace(/[NnEe]/g, "")
    .replace(/[SsWw]/g, "-")
    .match(/(-?\d+(?:\.\d+)?)\s*[, ]\s*(-?\d+(?:\.\d+)?)/);
  if (!m) return null;
  const lat = parseFloat(m[1]!);
  const lon = parseFloat(m[2]!);
  if (Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
  return { lat, lon };
}

const jsonCache = new Map<string, Promise<unknown>>();

/** Load + cache a JSON asset bundled alongside the app. Uses XHR, not fetch:
 *  Android WebView's fetch() rejects `file://` URLs, XHR handles them. */
export function loadJson<T>(url: string): Promise<T> {
  let p = jsonCache.get(url) as Promise<T> | undefined;
  if (!p) {
    p = new Promise<T>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("GET", url);
      xhr.responseType = "json";
      xhr.onload = () => {
        // file:// responses report status 0 but still deliver the body.
        if ((xhr.status === 0 || (xhr.status >= 200 && xhr.status < 300)) && xhr.response != null) {
          resolve(xhr.response as T);
        } else {
          reject(new Error(`${url}: HTTP ${xhr.status}`));
        }
      };
      xhr.onerror = () => reject(new Error(`${url}: request failed`));
      xhr.send();
    });
    jsonCache.set(url, p);
  }
  return p;
}
