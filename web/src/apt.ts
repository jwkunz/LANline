// NOAA APT (137 MHz weather satellite) client-side types + binary image
// decode. Mirrors the server's `apt::Status` JSON and `apt::Image::encode()`
// raster (see docs/rest-api.md).

export interface AptStatus {
  width: number;
  height: number;
  lines: number;
  sync_quality: number;
}

export interface AptImage {
  width: number;
  height: number;
  /** Row-major grayscale, one byte per pixel (channel A then channel B per row). */
  pixels: Uint8Array;
}

/** Parse `GET /api/v1/apt/image`'s `[u32 width LE][u32 height LE][bytes]`. */
export function parseAptImage(buf: ArrayBuffer): AptImage {
  const view = new DataView(buf);
  const width = buf.byteLength >= 4 ? view.getUint32(0, true) : 0;
  const height = buf.byteLength >= 8 ? view.getUint32(4, true) : 0;
  const want = width * height;
  const have = Math.max(0, buf.byteLength - 8);
  const pixels = new Uint8Array(buf, 8, Math.min(want, have));
  return { width, height, pixels };
}

export interface AptSatellite {
  name: string;
  freq_hz: number;
}

/** NOAA POES 137 MHz APT downlinks. Whether any of these is actually
 *  receivable right now depends on that specific satellite being above the
 *  horizon for a pass — unlike AM/FM/ADS-B/AIS, which all have a broadcaster
 *  (or many aircraft/vessels) continuously in range, APT is a real-time feed
 *  from one spacecraft with no store-and-forward. */
export const APT_SATELLITES: AptSatellite[] = [
  { name: "NOAA-19", freq_hz: 137_100_000 },
  { name: "NOAA-18", freq_hz: 137_912_500 },
  { name: "NOAA-15", freq_hz: 137_620_000 },
];

export const APT_MIN_HZ = 137_000_000;
export const APT_MAX_HZ = 138_000_000;
