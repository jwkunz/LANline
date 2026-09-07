// SSTV (slow-scan TV) client-side types + binary image decode. Mirrors the
// server's `sstv::Status` JSON and `sstv::Image::encode()` raster
// (see docs/rest-api.md).

export interface SstvStatus {
  width: number;
  height: number;
  mode: string;
  line: number;
  frames: number;
  snr_db: number;
  locked: boolean;
}

export interface SstvImage {
  width: number;
  height: number;
  /** Row-major RGBA, four bytes per pixel. */
  rgba: Uint8ClampedArray;
}

/** Parse `GET /api/v1/sstv/image`'s `[u32 width LE][u32 height LE][RGBA]`. */
export function parseSstvImage(buf: ArrayBuffer): SstvImage {
  const view = new DataView(buf);
  const width = buf.byteLength >= 4 ? view.getUint32(0, true) : 0;
  const height = buf.byteLength >= 8 ? view.getUint32(4, true) : 0;
  const want = width * height * 4;
  const have = Math.max(0, buf.byteLength - 8);
  const rgba = new Uint8ClampedArray(buf.slice(8, 8 + Math.min(want, have)));
  return { width, height, rgba };
}

/** Decoder modes offered in the picker (`auto` reads the VIS header). */
export const SSTV_MODES = [
  "auto",
  "robot36",
  "scottie1",
  "scottie2",
  "scottiedx",
  "martin1",
  "martin2",
  "pd120",
  "pd180",
] as const;

/** Quick-tune presets: [label, Hz, demod]. */
export const SSTV_PRESETS: [string, number, "usb" | "lsb" | "fm"][] = [
  ["ISS 145.800 (FM)", 145_800_000, "fm"],
  ["2 m FM calling 144.500", 144_500_000, "fm"],
  ["20 m 14.230 (USB)", 14_230_000, "usb"],
  ["20 m 14.233 (USB)", 14_233_000, "usb"],
  ["40 m 7.171 (LSB)", 7_171_000, "lsb"],
  ["80 m 3.845 (LSB)", 3_845_000, "lsb"],
];
