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

/** Output geometry (width, height) per mode key — must match
 *  `server/src/sstv/modes.rs`. Used to size the RGB payload for TX. */
export const SSTV_MODE_DIMS: Record<string, [number, number]> = {
  scottie1: [320, 256],
  scottie2: [320, 256],
  scottiedx: [320, 256],
  martin1: [320, 256],
  martin2: [320, 256],
  robot36: [320, 240],
  pd120: [640, 496],
  pd180: [640, 496],
};

/** Modes offered in the transmit picker (DX / PD 180 omitted — 60–190 s of
 *  on-air time — though the server will encode them if asked directly). */
export const SSTV_TX_MODES = [
  "scottie1",
  "scottie2",
  "martin1",
  "martin2",
  "robot36",
  "pd120",
] as const;

/** Draw an image source cover-fitted onto a `w×h` offscreen canvas and return
 *  its RGB bytes (row-major, alpha stripped) — the body for `POST /sstv/tx`. */
export function imageToRgb(source: CanvasImageSource, w: number, h: number): Uint8Array {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d canvas context");
  // cover fit: scale so the source fills w×h, centre-crop the overflow.
  const sw = (source as HTMLImageElement).naturalWidth || (source as HTMLVideoElement).videoWidth ||
    (source as HTMLCanvasElement).width;
  const sh = (source as HTMLImageElement).naturalHeight ||
    (source as HTMLVideoElement).videoHeight || (source as HTMLCanvasElement).height;
  const scale = Math.max(w / sw, h / sh);
  const dw = sw * scale;
  const dh = sh * scale;
  ctx.drawImage(source, (w - dw) / 2, (h - dh) / 2, dw, dh);
  const { data } = ctx.getImageData(0, 0, w, h);
  const rgb = new Uint8Array(w * h * 3);
  for (let i = 0, j = 0; i < data.length; i += 4, j += 3) {
    rgb[j] = data[i];
    rgb[j + 1] = data[i + 1];
    rgb[j + 2] = data[i + 2];
  }
  return rgb;
}

/** Quick-tune presets: [label, Hz, demod]. */
export const SSTV_PRESETS: [string, number, "usb" | "lsb" | "fm"][] = [
  ["ISS 145.800 (FM)", 145_800_000, "fm"],
  ["2 m FM calling 144.500", 144_500_000, "fm"],
  ["20 m 14.230 (USB)", 14_230_000, "usb"],
  ["20 m 14.233 (USB)", 14_233_000, "usb"],
  ["40 m 7.171 (LSB)", 7_171_000, "lsb"],
  ["80 m 3.845 (LSB)", 3_845_000, "lsb"],
];
