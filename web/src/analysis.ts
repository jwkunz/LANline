// Receiver Analysis: an interactive FFT panadapter + waterfall, GQRX-style.
//
// The server (analysis mode) computes windowed FFTs and serves them as a
// compact binary frame — a f32 averaged spectrum for the panadapter plus any
// new u8-quantised waterfall rows since our cursor. This module renders that
// to two canvases and layers the interactions on top: wheel to zoom the
// frequency axis, drag to pan, click to re-tune centre, adjustable dynamic
// range and colour map (all client-side over a fixed wide dB window), plus
// starting/stopping an IQ .wav recording.

import type { Client } from "./api";

const DB_MIN = -150; // matches analysis::DB_LO
const AXIS_PX = 44; // left gutter (dB labels) — kept in sync with draw()
const CENTER_STEPS_HZ = [10_000, 25_000, 100_000, 250_000, 500_000, 1_000_000, 2_500_000, 5_000_000];
const DB_MAX = 10; //   matches analysis::DB_HI
const WF_ROWS = 1024; // offscreen waterfall history height

type ColormapName = "turbo" | "viridis" | "gqrx" | "grayscale" | "inferno";

function buildLut(name: ColormapName): Uint8ClampedArray {
  // Control points [t, r, g, b] 0..1; linear interp to 256 entries.
  const stops: Record<ColormapName, [number, number, number, number][]> = {
    grayscale: [
      [0, 0, 0, 0],
      [1, 1, 1, 1],
    ],
    gqrx: [
      [0.0, 0, 0, 0.1],
      [0.2, 0, 0, 0.6],
      [0.45, 0, 0.7, 0.9],
      [0.7, 1, 1, 0],
      [1.0, 1, 0.2, 0],
    ],
    turbo: [
      [0.0, 0.19, 0.07, 0.23],
      [0.25, 0.16, 0.62, 0.98],
      [0.5, 0.35, 0.98, 0.47],
      [0.7, 0.9, 0.92, 0.2],
      [0.85, 0.98, 0.5, 0.12],
      [1.0, 0.6, 0.06, 0.02],
    ],
    viridis: [
      [0.0, 0.27, 0.0, 0.33],
      [0.35, 0.19, 0.4, 0.56],
      [0.6, 0.13, 0.63, 0.53],
      [0.85, 0.48, 0.82, 0.28],
      [1.0, 0.99, 0.91, 0.15],
    ],
    inferno: [
      [0.0, 0.0, 0.0, 0.02],
      [0.3, 0.34, 0.06, 0.43],
      [0.55, 0.73, 0.22, 0.33],
      [0.78, 0.98, 0.55, 0.04],
      [1.0, 0.99, 1.0, 0.64],
    ],
  };
  const s = stops[name];
  const lut = new Uint8ClampedArray(256 * 3);
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    let j = 0;
    while (j < s.length - 2 && t > s[j + 1]![0]) j++;
    const [t0, r0, g0, b0] = s[j]!;
    const [t1, r1, g1, b1] = s[j + 1]!;
    const f = t1 === t0 ? 0 : (t - t0) / (t1 - t0);
    lut[i * 3] = (r0 + (r1 - r0) * f) * 255;
    lut[i * 3 + 1] = (g0 + (g1 - g0) * f) * 255;
    lut[i * 3 + 2] = (b0 + (b1 - b0) * f) * 255;
  }
  return lut;
}

interface Frame {
  seq: number;
  centerHz: number;
  spanHz: number;
  nBins: number;
  dbLo: number;
  dbHi: number;
  avg: Float32Array;
  rows: Uint8Array[]; // each length nBins, oldest first
}

function decodeFrame(buf: ArrayBuffer): Frame | null {
  const dv = new DataView(buf);
  if (dv.byteLength < 38 || dv.getUint32(0, false) !== 0x4c574631) return null; // "LWF1"
  let o = 4;
  const seq = Number(dv.getBigUint64(o, true));
  o += 8;
  const centerHz = dv.getFloat64(o, true);
  o += 8;
  const spanHz = dv.getFloat64(o, true);
  o += 8;
  const nBins = dv.getUint32(o, true);
  o += 4;
  const dbLo = dv.getFloat32(o, true);
  o += 4;
  const dbHi = dv.getFloat32(o, true);
  o += 4;
  const nNew = dv.getUint32(o, true);
  o += 4;
  if (nBins === 0) return { seq, centerHz, spanHz, nBins, dbLo, dbHi, avg: new Float32Array(0), rows: [] };
  const avg = new Float32Array(buf.slice(o, o + nBins * 4));
  o += nBins * 4;
  const rows: Uint8Array[] = [];
  for (let i = 0; i < nNew && o + nBins <= dv.byteLength; i++) {
    rows.push(new Uint8Array(buf.slice(o, o + nBins)));
    o += nBins;
  }
  return { seq, centerHz, spanHz, nBins, dbLo, dbHi, avg, rows };
}

const fmtHz = (hz: number): string => {
  const a = Math.abs(hz);
  if (a >= 1e6) return `${(hz / 1e6).toFixed(a % 1e6 === 0 ? 0 : 3)} MHz`;
  if (a >= 1e3) return `${(hz / 1e3).toFixed(a % 1e3 === 0 ? 0 : 1)} kHz`;
  return `${hz.toFixed(0)} Hz`;
};

export class AnalysisView {
  /** The `#analysis-host` div the visible DOM lives in — `main.ts` compares
   *  it against a fresh `panels` rebuild to know when to `rehost`. */
  host: HTMLElement;
  private client: Client;
  private onRetune: (hz: number) => void;

  private pan!: HTMLCanvasElement;
  private wf!: HTMLCanvasElement;
  private readout!: HTMLElement;
  private recBtn!: HTMLButtonElement;
  private recInfo!: HTMLElement;

  // offscreen waterfall history (nBins wide, WF_ROWS tall); newest row at bottom
  private wfBmp = document.createElement("canvas");
  private wfCtx = this.wfBmp.getContext("2d")!;
  private rowImg: ImageData | null = null;

  private centerHz = 0;
  private spanHz = 0;
  private nBins = 0;
  private avg: Float32Array = new Float32Array(0);
  private maxHold: Float32Array | null = null;
  private lastSeq = 0;
  private raf = 0;
  private inflight = false;
  private stopped = true;

  // view / display state (persisted in localStorage)
  private viewLo = -0.5; // fraction of span, -0.5..+0.5
  private viewHi = 0.5;
  private dbFloor = -110;
  private dbCeil = -20;
  private cmap: ColormapName = "turbo";
  private lut = buildLut("turbo");
  private showMaxHold = false;
  private cursorX: number | null = null;
  // active pointers on the plot (for pan + pinch-zoom)
  private ptrs = new Map<number, number>(); // pointerId → clientX
  private panLastX: number | null = null;
  private panMoved = 0;
  private pinchStartDist = 0;
  private pinchAnchor = 0.5;
  private pinchViewLo = 0;
  private pinchViewHi = 0;
  private centerStepHz = 500_000;
  private fBox!: HTMLInputElement;

  constructor(
    host: HTMLElement,
    client: Client,
    onRetune: (hz: number) => void,
    initialCenterHz = 0,
  ) {
    this.host = host;
    this.client = client;
    this.onRetune = onRetune;
    this.centerHz = initialCenterHz;
    this.loadPrefs();
    this.build();
  }

  // ---- lifecycle --------------------------------------------------
  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.lastSeq = 0;
    this.tick();
  }

  stop(): void {
    this.stopped = true;
    cancelAnimationFrame(this.raf);
  }

  destroy(): void {
    this.stop();
    this.host.replaceChildren();
  }

  /** A `panels` rebuild replaced our host div with a fresh empty one. Move
   *  the visible DOM over; the offscreen waterfall and view state are class
   *  fields, so they carry across untouched. */
  rehost(host: HTMLElement): void {
    this.host = host;
    this.build();
    // sync the freshly-built controls to current state
    (this.host.querySelector(".an-fft") as HTMLSelectElement | null)?.blur();
  }

  // ---- DOM ------------------------------------------------------
  private build(): void {
    this.host.replaceChildren();
    this.host.className = "an-root";
    this.host.innerHTML = `
      <div class="an-freq">
        <label for="anf">Centre</label>
        <button class="secondary an-f-dn" title="down one step">−</button>
        <input id="anf" type="text" inputmode="decimal" class="an-f" />
        <span class="an-unit">MHz</span>
        <button class="secondary an-f-up" title="up one step">+</button>
        <select class="an-step" title="tune step">
          ${CENTER_STEPS_HZ.map(
            (s) =>
              `<option value="${s}"${s === this.centerStepHz ? " selected" : ""}>${
                s >= 1e6 ? s / 1e6 + " MHz" : s / 1e3 + " kHz"
              }</option>`,
          ).join("")}
        </select>
      </div>
      <div class="an-plots">
        <canvas class="an-pan"></canvas>
        <canvas class="an-wf"></canvas>
      </div>
      <div class="an-readout note"></div>
      <p class="note an-hint">Wheel / pinch to zoom · drag to pan · click the spectrum to set centre.</p>
      <div class="an-controls">
        <label>Range
          <input type="number" class="an-floor" step="5" value="${this.dbFloor}"> …
          <input type="number" class="an-ceil" step="5" value="${this.dbCeil}"> dBFS</label>
        <label>Colour
          <select class="an-cmap">
            ${(["turbo", "viridis", "inferno", "gqrx", "grayscale"] as ColormapName[])
              .map((c) => `<option value="${c}"${c === this.cmap ? " selected" : ""}>${c}</option>`)
              .join("")}
          </select></label>
        <label class="an-check"><input type="checkbox" class="an-hold"${this.showMaxHold ? " checked" : ""}> max-hold</label>
        <button class="secondary an-reset">reset zoom</button>
      </div>
      <div class="an-controls">
        <label>FFT <select class="an-fft">${[1024, 2048, 4096, 8192, 16384]
          .map((n) => `<option value="${n}">${n}</option>`)
          .join("")}</select></label>
        <label>Window <select class="an-win">
          ${["Hann", "Blackman", "Blackman-Harris", "Rect"].map((w, i) => `<option value="${i}">${w}</option>`).join("")}
        </select></label>
        <label>Rate <select class="an-rate">${[5, 10, 15, 20, 30, 45, 60]
          .map((r) => `<option value="${r}"${r === 20 ? " selected" : ""}>${r} fps</option>`)
          .join("")}</select></label>
      </div>
      <div class="an-controls">
        <button class="an-rec">⏺ Record IQ</button>
        <span class="an-rec-info note"></span>
      </div>`;

    this.pan = this.host.querySelector(".an-pan")!;
    this.wf = this.host.querySelector(".an-wf")!;
    this.readout = this.host.querySelector(".an-readout")!;
    this.recBtn = this.host.querySelector(".an-rec")!;
    this.recInfo = this.host.querySelector(".an-rec-info")!;
    this.fBox = this.host.querySelector(".an-f")!;
    this.fBox.value = this.centerHz ? (this.centerHz / 1e6).toFixed(4) : "";

    const q = <T extends Element>(s: string) => this.host.querySelector<T>(s)!;

    // --- centre frequency ---
    const stepCenter = (dir: number) => this.setCenter(this.centerHz + dir * this.centerStepHz);
    q<HTMLButtonElement>(".an-f-dn").addEventListener("click", () => stepCenter(-1));
    q<HTMLButtonElement>(".an-f-up").addEventListener("click", () => stepCenter(1));
    q<HTMLSelectElement>(".an-step").addEventListener("change", (e) => {
      this.centerStepHz = +(e.target as HTMLSelectElement).value;
    });
    const commitBox = () => {
      const mhz = parseFloat(this.fBox.value.replace(/[, ]/g, ""));
      if (Number.isFinite(mhz) && mhz > 0) this.setCenter(Math.round(mhz * 1e6));
    };
    this.fBox.addEventListener("change", commitBox);
    this.fBox.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        commitBox();
        this.fBox.blur();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        stepCenter(1);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        stepCenter(-1);
      }
    });
    q<HTMLInputElement>(".an-floor").addEventListener("input", (e) => {
      this.dbFloor = +(e.target as HTMLInputElement).value;
      this.savePrefs();
    });
    q<HTMLInputElement>(".an-ceil").addEventListener("input", (e) => {
      this.dbCeil = +(e.target as HTMLInputElement).value;
      this.savePrefs();
    });
    q<HTMLSelectElement>(".an-cmap").addEventListener("change", (e) => {
      this.cmap = (e.target as HTMLSelectElement).value as ColormapName;
      this.lut = buildLut(this.cmap);
      this.savePrefs();
    });
    q<HTMLInputElement>(".an-hold").addEventListener("change", (e) => {
      this.showMaxHold = (e.target as HTMLInputElement).checked;
      this.maxHold = null;
      this.savePrefs();
    });
    q<HTMLButtonElement>(".an-reset").addEventListener("click", () => {
      this.viewLo = -0.5;
      this.viewHi = 0.5;
    });
    q<HTMLSelectElement>(".an-fft").addEventListener("change", (e) =>
      this.patchParams({ fft_size: +(e.target as HTMLSelectElement).value }),
    );
    q<HTMLSelectElement>(".an-win").addEventListener("change", (e) =>
      this.patchParams({ window: +(e.target as HTMLSelectElement).value }),
    );
    q<HTMLSelectElement>(".an-rate").addEventListener("change", (e) =>
      this.patchParams({ frame_rate_hz: +(e.target as HTMLSelectElement).value }),
    );
    this.recBtn.addEventListener("click", () => this.toggleRecord());

    for (const c of [this.pan, this.wf]) {
      c.addEventListener("wheel", (e) => this.onWheel(e), { passive: false });
      c.addEventListener("pointerdown", (e) => this.onDown(e));
      c.addEventListener("pointermove", (e) => this.onMove(e));
      c.addEventListener("pointerup", (e) => this.onUp(e));
      c.addEventListener("pointercancel", (e) => this.onUp(e));
      c.addEventListener("pointerleave", () => {
        this.cursorX = null;
        this.readout.textContent = "";
      });
      c.addEventListener("dblclick", () => {
        this.viewLo = -0.5;
        this.viewHi = 0.5;
      });
    }
  }

  /** Re-tune the receiver centre; optimistically update locally so the dial
   *  and marker move immediately, the next frame confirms. */
  private setCenter(hz: number): void {
    hz = Math.round(hz);
    if (!Number.isFinite(hz) || hz <= 0) return;
    this.centerHz = hz;
    if (this.fBox && document.activeElement !== this.fBox) this.fBox.value = (hz / 1e6).toFixed(4);
    this.onRetune(hz);
  }

  private async patchParams(p: Record<string, number>): Promise<void> {
    try {
      await this.client.patchRadio({ mode_params: p });
      this.lastSeq = 0; // geometry may change → resync
    } catch {
      /* transient */
    }
  }

  // ---- interaction ------------------------------------------------
  /** Cursor position as a fraction 0..1 across the *plotted* area (i.e. past
   *  the left dB-label gutter). */
  private frac(e: { clientX: number }, c: HTMLCanvasElement): number {
    const r = c.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - r.left - AXIS_PX) / Math.max(1, r.width - AXIS_PX)));
  }
  private viewHzAt(f: number): number {
    return this.centerHz + (this.viewLo + (this.viewHi - this.viewLo) * f) * this.spanHz;
  }
  private onWheel(e: WheelEvent): void {
    e.preventDefault();
    const p = this.frac(e, e.currentTarget as HTMLCanvasElement);
    const anchor = this.viewLo + (this.viewHi - this.viewLo) * p;
    const k = e.deltaY > 0 ? 1.25 : 0.8;
    let lo = anchor + (this.viewLo - anchor) * k;
    let hi = anchor + (this.viewHi - anchor) * k;
    if (hi - lo < 0.01) return;
    [lo, hi] = this.clampView(lo, hi);
    this.viewLo = lo;
    this.viewHi = hi;
  }
  private onDown(e: PointerEvent): void {
    const c = e.currentTarget as HTMLElement;
    c.setPointerCapture(e.pointerId);
    this.ptrs.set(e.pointerId, e.clientX);
    if (this.ptrs.size === 1) {
      this.panLastX = e.clientX;
      this.panMoved = 0;
    } else if (this.ptrs.size === 2) {
      const [a, b] = [...this.ptrs.values()];
      this.pinchStartDist = Math.max(1, Math.abs(a! - b!));
      this.pinchViewLo = this.viewLo;
      this.pinchViewHi = this.viewHi;
      this.pinchAnchor = this.frac({ clientX: (a! + b!) / 2 }, e.currentTarget as HTMLCanvasElement);
      this.panLastX = null; // suspend pan while pinching
    }
  }

  private onMove(e: PointerEvent): void {
    const c = e.currentTarget as HTMLCanvasElement;
    this.cursorX = this.frac(e, c);
    if (this.ptrs.has(e.pointerId)) this.ptrs.set(e.pointerId, e.clientX);

    if (this.ptrs.size >= 2) {
      const [a, b] = [...this.ptrs.values()];
      const dist = Math.max(1, Math.abs(a! - b!));
      const k = this.pinchStartDist / dist; // fingers apart → k<1 → zoom in
      const span0 = this.pinchViewHi - this.pinchViewLo;
      const anchorHzFrac = this.pinchViewLo + span0 * this.pinchAnchor;
      let span = Math.min(1, Math.max(0.01, span0 * k));
      let lo = anchorHzFrac - span * this.pinchAnchor;
      let hi = lo + span;
      [lo, hi] = this.clampView(lo, hi);
      this.viewLo = lo;
      this.viewHi = hi;
    } else if (this.panLastX != null) {
      const r = c.getBoundingClientRect();
      const dx = e.clientX - this.panLastX;
      this.panMoved += Math.abs(dx);
      this.panLastX = e.clientX;
      const d = (dx / Math.max(1, r.width - AXIS_PX)) * (this.viewHi - this.viewLo);
      const [lo, hi] = this.clampView(this.viewLo - d, this.viewHi - d);
      this.viewLo = lo;
      this.viewHi = hi;
    }

    const hz = this.viewHzAt(this.cursorX);
    const db = this.dbAt(this.cursorX);
    this.readout.textContent = `${fmtHz(hz)}   ${db == null ? "" : db.toFixed(1) + " dBFS"}`;
  }

  private onUp(e: PointerEvent): void {
    const wasSingle = this.ptrs.size === 1;
    this.ptrs.delete(e.pointerId);
    if (this.ptrs.size === 1) {
      // dropped to one finger after a pinch — reseat pan origin
      this.panLastX = [...this.ptrs.values()][0]!;
      this.panMoved = 999;
    } else if (this.ptrs.size === 0) {
      const tap = wasSingle && this.panMoved < 8;
      this.panLastX = null;
      if (tap && this.spanHz > 0) {
        this.setCenter(this.viewHzAt(this.frac(e, e.currentTarget as HTMLCanvasElement)));
      }
    }
  }

  private clampView(lo: number, hi: number): [number, number] {
    const span = hi - lo;
    if (span >= 1) return [-0.5, 0.5];
    if (lo < -0.5) return [-0.5, -0.5 + span];
    if (hi > 0.5) return [0.5 - span, 0.5];
    return [lo, hi];
  }
  private dbAt(f: number): number | null {
    if (!this.avg.length) return null;
    const bin = Math.round((this.viewLo + (this.viewHi - this.viewLo) * f + 0.5) * (this.nBins - 1));
    return this.avg[Math.min(this.nBins - 1, Math.max(0, bin))] ?? null;
  }

  // ---- recording ------------------------------------------------
  private recording = false;
  private async toggleRecord(): Promise<void> {
    try {
      if (!this.recording) {
        await this.client.analysisRecord("start", 60);
        this.recording = true;
        this.recBtn.textContent = "⏹ Stop recording";
        this.recBtn.classList.add("an-rec-on");
        this.recInfo.textContent = "recording… (≈8 MB/s, 60s cap)";
      } else {
        const r = (await this.client.analysisRecord("stop")) as {
          last_recording?: { filename: string; bytes: number; secs: number };
        };
        this.recording = false;
        this.recBtn.textContent = "⏺ Record IQ";
        this.recBtn.classList.remove("an-rec-on");
        const lr = r.last_recording;
        if (lr) {
          const mb = (lr.bytes / 1e6).toFixed(1);
          this.recInfo.innerHTML = `saved <b>${lr.filename}</b> (${lr.secs.toFixed(1)}s, ${mb} MB) — <a href="${this.client.analysisRecordingUrl()}" download>download</a>`;
        } else {
          this.recInfo.textContent = "stopped.";
        }
      }
    } catch (e) {
      this.recInfo.textContent = `record failed — ${(e as Error).message}`;
    }
  }

  // ---- render loop ---------------------------------------------
  private tick = async (): Promise<void> => {
    if (this.stopped) return;
    if (!this.inflight) {
      this.inflight = true;
      this.client
        .analysisSpectrum(this.lastSeq, 240)
        .then((buf) => this.ingest(buf))
        .catch(() => {})
        .finally(() => (this.inflight = false));
    }
    this.draw();
    this.raf = requestAnimationFrame(() => this.tick());
  };

  private ingest(buf: ArrayBuffer): void {
    const f = decodeFrame(buf);
    if (!f || f.nBins === 0) return;

    if (f.nBins !== this.nBins || Math.abs(f.spanHz - this.spanHz) > 1) {
      this.nBins = f.nBins;
      this.wfBmp.width = f.nBins;
      this.wfBmp.height = WF_ROWS;
      this.wfCtx.fillStyle = "#000";
      this.wfCtx.fillRect(0, 0, f.nBins, WF_ROWS);
      this.rowImg = this.wfCtx.createImageData(f.nBins, 1);
      this.maxHold = null;
    }
    this.centerHz = f.centerHz;
    this.spanHz = f.spanHz;
    this.avg = f.avg;
    if (this.fBox && document.activeElement !== this.fBox) {
      this.fBox.value = (f.centerHz / 1e6).toFixed(4);
    }
    if (this.showMaxHold) {
      if (!this.maxHold || this.maxHold.length !== f.nBins) this.maxHold = f.avg.slice();
      else for (let i = 0; i < f.nBins; i++) this.maxHold[i] = Math.max(this.maxHold[i]!, f.avg[i]!);
    }

    if (f.seq < this.lastSeq) this.lastSeq = 0; // pipeline restarted
    if (f.rows.length) {
      // scroll history up by rows.length, blit new rows at the bottom
      const k = Math.min(f.rows.length, WF_ROWS);
      this.wfCtx.drawImage(this.wfBmp, 0, 0, f.nBins, WF_ROWS, 0, -k, f.nBins, WF_ROWS);
      const img = this.rowImg!;
      for (let r = 0; r < k; r++) {
        const row = f.rows[f.rows.length - k + r]!;
        const d = img.data;
        for (let x = 0; x < f.nBins; x++) {
          // u8 over DB_MIN..DB_MAX → remap to the visible floor..ceil
          const dbv = DB_MIN + (row[x]! / 255) * (DB_MAX - DB_MIN);
          const t = Math.min(255, Math.max(0, ((dbv - this.dbFloor) / (this.dbCeil - this.dbFloor)) * 255)) | 0;
          d[x * 4] = this.lut[t * 3]!;
          d[x * 4 + 1] = this.lut[t * 3 + 1]!;
          d[x * 4 + 2] = this.lut[t * 3 + 2]!;
          d[x * 4 + 3] = 255;
        }
        this.wfCtx.putImageData(img, 0, WF_ROWS - k + r);
      }
    }
    this.lastSeq = f.seq;
  }

  private fitCanvas(c: HTMLCanvasElement, cssH: number): [number, number, number] {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = c.clientWidth || 600;
    if (c.width !== Math.round(w * dpr) || c.height !== Math.round(cssH * dpr)) {
      c.width = Math.round(w * dpr);
      c.height = Math.round(cssH * dpr);
      c.style.height = `${cssH}px`;
    }
    return [c.width, c.height, dpr];
  }

  private draw(): void {
    if (!this.spanHz) return;
    const axis = 44; // left gutter for dB labels
    const lo = this.viewLo + 0.5,
      hi = this.viewHi + 0.5; // 0..1 fraction of full span

    // --- panadapter ---
    {
      const [W, H] = this.fitCanvas(this.pan, 150);
      const g = this.pan.getContext("2d")!;
      g.clearRect(0, 0, W, H);
      g.fillStyle = getCss("--card");
      g.fillRect(0, 0, W, H);
      const plotW = W - axis * (W / this.pan.clientWidth);
      const gx = axis * (W / this.pan.clientWidth);
      // dB grid
      g.strokeStyle = getCss("--line");
      g.fillStyle = getCss("--muted");
      g.font = `${11 * (W / this.pan.clientWidth)}px system-ui`;
      g.textAlign = "right";
      g.lineWidth = 1;
      const range = this.dbCeil - this.dbFloor;
      const stepDb = range > 80 ? 20 : range > 40 ? 10 : 5;
      for (let d = Math.ceil(this.dbFloor / stepDb) * stepDb; d <= this.dbCeil; d += stepDb) {
        const y = H - ((d - this.dbFloor) / range) * H;
        g.beginPath();
        g.moveTo(gx, y);
        g.lineTo(W, y);
        g.stroke();
        g.fillText(`${d}`, gx - 4, y + 4);
      }
      const trace = (data: Float32Array, fill: boolean, color: string) => {
        g.beginPath();
        for (let px = 0; px <= plotW; px++) {
          const fr = lo + (hi - lo) * (px / plotW);
          const bin = Math.min(this.nBins - 1, Math.max(0, fr * (this.nBins - 1)));
          const i = bin | 0;
          const v = data[i]! + (data[Math.min(this.nBins - 1, i + 1)]! - data[i]!) * (bin - i);
          const y = H - ((v - this.dbFloor) / range) * H;
          if (px === 0) g.moveTo(gx + px, y);
          else g.lineTo(gx + px, y);
        }
        if (fill) {
          g.lineTo(W, H);
          g.lineTo(gx, H);
          g.closePath();
          g.fillStyle = color;
          g.fill();
        } else {
          g.strokeStyle = color;
          g.lineWidth = 1.2;
          g.stroke();
        }
      };
      if (this.avg.length) {
        trace(this.avg, true, mix(getCss("--accent"), 0.22));
        trace(this.avg, false, getCss("--accent"));
      }
      if (this.showMaxHold && this.maxHold) trace(this.maxHold, false, getCss("--muted"));
      const fc = (0 - this.viewLo) / (this.viewHi - this.viewLo);
      if (fc >= 0 && fc <= 1) {
        const x = gx + fc * plotW;
        g.strokeStyle = "#e0463b";
        g.lineWidth = 1.5;
        g.beginPath();
        g.moveTo(x, 0);
        g.lineTo(x, H);
        g.stroke();
      }
      if (this.cursorX != null) {
        const x = gx + this.cursorX * plotW;
        g.strokeStyle = getCss("--muted");
        g.setLineDash([4, 3]);
        g.beginPath();
        g.moveTo(x, 0);
        g.lineTo(x, H);
        g.stroke();
        g.setLineDash([]);
      }
    }

    // --- waterfall ---
    {
      const [W, H, dpr] = this.fitCanvas(this.wf, 260);
      const g = this.wf.getContext("2d")!;
      g.imageSmoothingEnabled = false;
      g.fillStyle = "#000";
      g.fillRect(0, 0, W, H);
      const gx = axis * dpr;
      const sx = lo * this.nBins;
      const sw = (hi - lo) * this.nBins;
      g.drawImage(this.wfBmp, sx, 0, sw, WF_ROWS, gx, 0, W - gx, H);
      // frequency labels along the bottom
      g.fillStyle = getCss("--muted");
      g.font = `${11 * dpr}px system-ui`;
      g.textAlign = "center";
      const ticks = 6;
      for (let t = 0; t <= ticks; t++) {
        const fx = t / ticks;
        const x = gx + fx * (W - gx);
        const hz = this.centerHz + (this.viewLo + (this.viewHi - this.viewLo) * fx) * this.spanHz;
        g.fillStyle = "rgba(0,0,0,0.55)";
        const label = fmtHz(hz);
        const tw = g.measureText(label).width;
        g.fillRect(x - tw / 2 - 3, H - 16 * dpr, tw + 6, 15 * dpr);
        g.fillStyle = getCss("--muted");
        g.fillText(label, x, H - 5 * dpr);
        g.strokeStyle = "rgba(255,255,255,0.15)";
        g.beginPath();
        g.moveTo(x, 0);
        g.lineTo(x, H - 18 * dpr);
        g.stroke();
      }
      const fc = (0 - this.viewLo) / (this.viewHi - this.viewLo);
      if (fc >= 0 && fc <= 1) {
        const x = gx + fc * (W - gx);
        g.strokeStyle = "#e0463b";
        g.lineWidth = 1.5 * dpr;
        g.beginPath();
        g.moveTo(x, 0);
        g.lineTo(x, H);
        g.stroke();
      }
      if (this.cursorX != null) {
        const x = gx + this.cursorX * (W - gx);
        g.strokeStyle = "rgba(255,255,255,0.6)";
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(x, 0);
        g.lineTo(x, H);
        g.stroke();
      }
    }
  }

  // ---- prefs ---------------------------------------------------
  private loadPrefs(): void {
    try {
      const p = JSON.parse(localStorage.getItem("lanline.analysis") ?? "{}");
      if (typeof p.dbFloor === "number") this.dbFloor = p.dbFloor;
      if (typeof p.dbCeil === "number") this.dbCeil = p.dbCeil;
      if (typeof p.cmap === "string") {
        this.cmap = p.cmap;
        this.lut = buildLut(this.cmap);
      }
      if (typeof p.showMaxHold === "boolean") this.showMaxHold = p.showMaxHold;
    } catch {
      /* defaults */
    }
  }
  private savePrefs(): void {
    try {
      localStorage.setItem(
        "lanline.analysis",
        JSON.stringify({
          dbFloor: this.dbFloor,
          dbCeil: this.dbCeil,
          cmap: this.cmap,
          showMaxHold: this.showMaxHold,
        }),
      );
    } catch {
      /* private mode */
    }
  }
}

// --- small CSS-var helpers (waterfall wants concrete colours) --------
let cssCache: Record<string, string> = {};
function getCss(v: string): string {
  if (!(v in cssCache)) {
    cssCache[v] = getComputedStyle(document.documentElement).getPropertyValue(v).trim() || "#888";
  }
  return cssCache[v]!;
}
function mix(hex: string, alpha: number): string {
  // hex like "#2563eb" → rgba
  const h = hex.replace("#", "");
  const n = parseInt(h.length === 3 ? h.replace(/(.)/g, "$1$1") : h, 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}
export function clearAnalysisCssCache(): void {
  cssCache = {};
}
