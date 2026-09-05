// "Radio options" — a modal for the raw SoapySDR tuner knobs (PPM correction,
// per-element gain, analog bandwidth, sample rate, antenna, DC-offset mode,
// and free-form device settings). Reads the selected device's real ranges
// from GET /api/v1/device and PATCHes /api/v1/radio; only the controls the
// user actually touches are sent. Sample-rate / bandwidth / PPM / correction
// changes briefly restart the receiver (the server decides — see apply_patch).

import type { Client } from "./api";
import type { DeviceInfo, RadioConfig, Range, TunerConfig } from "./types";

const esc = (s: string): string =>
  s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

const num = (v: unknown, fallback = 0): number => {
  const n = typeof v === "number" ? v : parseFloat(String(v));
  return Number.isFinite(n) ? n : fallback;
};

/** A slider step that keeps a range usable even when the driver reports 0. */
function stepFor(r: Range): number {
  if (r.step && r.step > 0) return r.step;
  const span = Math.abs(r.max - r.min);
  return span > 0 ? span / 100 : 1;
}

function hz(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(n % 1e6 === 0 ? 0 : 3)} MHz`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n % 1e3 === 0 ? 0 : 1)} kHz`;
  return `${n} Hz`;
}

let openEl: HTMLElement | null = null;

export function radioOptionsOpen(): boolean {
  return openEl != null;
}

export async function openRadioOptions(
  client: Client,
  radio: RadioConfig,
  onApplied: (updated: RadioConfig) => void,
): Promise<void> {
  if (openEl) return;

  const dev: DeviceInfo | null = await client.device().catch(() => null);
  const t = radio.tuner as unknown as TunerConfig;
  const rx = dev?.rx;

  const changed = new Set<string>();
  const mark = (k: string) => changed.add(k);

  // ---- build sections -------------------------------------------------
  const sections: string[] = [];

  // Frequency correction (software ppm — always available).
  sections.push(`
    <div class="ro-group">
      <label class="ro-label" for="ro-ppm">Frequency correction</label>
      <div class="ro-row">
        <input type="number" id="ro-ppm" step="0.1" value="${num(t.freq_correction_ppm)}" />
        <span class="ro-unit">ppm</span>
      </div>
      <p class="note">Applied in software to every tune. Negative = the crystal
      runs high. See docs/architecture.md#hackrf-frequency-calibration.</p>
    </div>`);

  // Gain.
  {
    const elems = rx?.gain_elements ?? [];
    const agc = t.gain_mode === "agc";
    const sliders = elems
      .map((e) => {
        const cur = num(t.gain_elements_db?.[e.name], e.range_db.min);
        const s = stepFor(e.range_db);
        return `
        <div class="ro-slider" data-el="${esc(e.name)}">
          <div class="ro-slider-head">
            <span>${esc(e.name)}</span>
            <output>${cur.toFixed(s < 1 ? 1 : 0)} dB</output>
          </div>
          <input type="range" min="${e.range_db.min}" max="${e.range_db.max}"
                 step="${s}" value="${cur}" ${agc ? "disabled" : ""} />
          <div class="ro-slider-scale"><span>${e.range_db.min}</span><span>${e.range_db.max} dB</span></div>
        </div>`;
      })
      .join("");
    sections.push(`
      <div class="ro-group" id="ro-gain">
        <label class="ro-label">Gain</label>
        ${
          rx?.has_agc
            ? `<label class="ro-check"><input type="checkbox" id="ro-agc" ${agc ? "checked" : ""} /> Automatic (AGC)</label>`
            : ""
        }
        ${sliders || `<p class="note">This device reports no adjustable gain elements.</p>`}
      </div>`);
  }

  // Analog bandwidth.
  if (rx?.bandwidth_ranges_hz?.length) {
    const r = rx.bandwidth_ranges_hz;
    const lo = Math.min(...r.map((x) => x.min));
    const hi = Math.max(...r.map((x) => x.max));
    const auto = t.bandwidth_hz == null;
    const cur = num(t.bandwidth_hz, lo);
    const discrete = r.filter((x) => x.min === x.max).map((x) => x.min);
    const field = discrete.length
      ? `<select id="ro-bw-input" ${auto ? "disabled" : ""}>${[...new Set([cur, ...discrete])]
          .sort((a, b) => a - b)
          .map((v) => `<option value="${v}" ${v === cur ? "selected" : ""}>${hz(v)}</option>`)
          .join("")}</select>`
      : `<div class="ro-row"><input type="number" id="ro-bw-input" step="1000" min="${lo}" max="${hi}"
           value="${cur}" ${auto ? "disabled" : ""} /><span class="ro-unit">Hz</span></div>`;
    sections.push(`
      <div class="ro-group" id="ro-bw">
        <label class="ro-label" for="ro-bw-input">Analog filter bandwidth</label>
        <label class="ro-check"><input type="checkbox" id="ro-bw-auto" ${auto ? "checked" : ""} /> Auto (driver default)</label>
        ${field}
        <p class="note">Range ${hz(lo)}–${hz(hi)}. Restarts the receiver.</p>
      </div>`);
  }

  // Sample rate.
  if (rx?.sample_rate_ranges_hz?.length) {
    const r = rx.sample_rate_ranges_hz;
    // Discrete rates come through as zero-width ranges; a real range gets a box.
    const discrete = r.filter((x) => x.min === x.max).map((x) => x.min);
    const cur = num(t.sample_rate_hz);
    const body = discrete.length
      ? `<select id="ro-rate">${[...new Set([cur, ...discrete])]
          .sort((a, b) => a - b)
          .map((v) => `<option value="${v}" ${v === cur ? "selected" : ""}>${hz(v)}</option>`)
          .join("")}</select>`
      : `<div class="ro-row"><input type="number" id="ro-rate" step="250000"
           min="${Math.min(...r.map((x) => x.min))}" max="${Math.max(...r.map((x) => x.max))}"
           value="${cur}" /><span class="ro-unit">Hz</span></div>`;
    sections.push(`
      <div class="ro-group">
        <label class="ro-label" for="ro-rate">Sample rate</label>
        ${body}
        <p class="note">Changing this restarts the receiver (brief audio gap).</p>
      </div>`);
  }

  // Antenna.
  if (rx?.antennas && rx.antennas.length > 1) {
    const cur = t.antenna ?? rx.antennas[0];
    sections.push(`
      <div class="ro-group">
        <label class="ro-label" for="ro-ant">Antenna</label>
        <select id="ro-ant">${rx.antennas
          .map((a) => `<option value="${esc(a)}" ${a === cur ? "selected" : ""}>${esc(a)}</option>`)
          .join("")}</select>
      </div>`);
  }

  // DC-offset correction.
  if (rx?.has_dc_offset_mode) {
    sections.push(`
      <div class="ro-group">
        <label class="ro-check"><input type="checkbox" id="ro-dc" ${t.dc_offset_correction ? "checked" : ""} />
        Automatic DC-offset correction</label>
      </div>`);
  }

  // Free-form device settings.
  {
    const rows = Object.entries(t.device_settings ?? {});
    const rowHtml = (k = "", v = "") => `
      <div class="ro-kv">
        <input type="text" class="ro-kv-k" placeholder="key" value="${esc(k)}" />
        <input type="text" class="ro-kv-v" placeholder="value" value="${esc(v)}" />
        <button class="secondary ro-kv-del" title="remove">✕</button>
      </div>`;
    sections.push(`
      <div class="ro-group" id="ro-settings">
        <label class="ro-label">Device settings <span class="note">(SoapySDR writeSetting — driver-specific keys)</span></label>
        <div id="ro-kv-list">${rows.map(([k, v]) => rowHtml(k, String(v))).join("") || ""}</div>
        <button class="secondary" id="ro-kv-add">+ add setting</button>
      </div>`);
  }

  const backdrop = document.createElement("div");
  backdrop.className = "ro-backdrop";
  backdrop.innerHTML = `
    <div class="ro-modal" role="dialog" aria-label="Radio options">
      <div class="ro-modal-head">
        <h2>Radio options</h2>
        <span class="note">${esc(dev?.label ?? "no device")}${dev ? ` · ${esc(dev.driver)}` : ""}</span>
      </div>
      <div class="ro-modal-body">
        ${dev ? sections.join("") : `<p class="note">No SDR selected — nothing to configure.</p>`}
      </div>
      <div class="ro-modal-foot">
        <span class="note" id="ro-msg"></span>
        <span style="flex:1"></span>
        <button class="secondary" id="ro-cancel">Close</button>
        <button id="ro-apply" ${dev ? "" : "disabled"}>Apply</button>
      </div>
    </div>`;
  document.body.appendChild(backdrop);
  openEl = backdrop;

  // ---- wiring -------------------------------------------------------
  const q = <T extends Element>(sel: string) => backdrop.querySelector<T>(sel);
  const qa = <T extends Element>(sel: string) => Array.from(backdrop.querySelectorAll<T>(sel));

  q<HTMLInputElement>("#ro-ppm")?.addEventListener("input", () => mark("freq_correction_ppm"));

  const agcBox = q<HTMLInputElement>("#ro-agc");
  agcBox?.addEventListener("change", () => {
    mark("gain");
    qa<HTMLInputElement>('#ro-gain input[type="range"]').forEach((s) => (s.disabled = agcBox.checked));
  });
  qa<HTMLInputElement>('#ro-gain input[type="range"]').forEach((s) => {
    const out = s.parentElement?.querySelector("output");
    s.addEventListener("input", () => {
      mark("gain");
      if (out) out.textContent = `${parseFloat(s.value).toFixed(Number(s.step) < 1 ? 1 : 0)} dB`;
    });
  });

  const bwAuto = q<HTMLInputElement>("#ro-bw-auto");
  const bwInput = q<HTMLInputElement | HTMLSelectElement>("#ro-bw-input");
  bwAuto?.addEventListener("change", () => {
    mark("bandwidth_hz");
    if (bwInput) bwInput.disabled = bwAuto.checked;
  });
  bwInput?.addEventListener("input", () => mark("bandwidth_hz"));

  q<HTMLSelectElement | HTMLInputElement>("#ro-rate")?.addEventListener("change", () => mark("sample_rate_hz"));
  q<HTMLSelectElement>("#ro-ant")?.addEventListener("change", () => mark("antenna"));
  q<HTMLInputElement>("#ro-dc")?.addEventListener("change", () => mark("dc_offset_correction"));

  const kvList = q<HTMLElement>("#ro-kv-list");
  const kvRow = (k = "", v = ""): HTMLElement => {
    const d = document.createElement("div");
    d.className = "ro-kv";
    d.innerHTML = `<input type="text" class="ro-kv-k" placeholder="key" value="${esc(k)}" />
      <input type="text" class="ro-kv-v" placeholder="value" value="${esc(v)}" />
      <button class="secondary ro-kv-del" title="remove">✕</button>`;
    d.querySelectorAll("input").forEach((i) => i.addEventListener("input", () => mark("device_settings")));
    d.querySelector(".ro-kv-del")?.addEventListener("click", () => {
      d.remove();
      mark("device_settings");
    });
    return d;
  };
  qa<HTMLInputElement>("#ro-kv-list input").forEach((i) => i.addEventListener("input", () => mark("device_settings")));
  qa<HTMLButtonElement>("#ro-kv-list .ro-kv-del").forEach((b) =>
    b.addEventListener("click", () => {
      b.closest(".ro-kv")?.remove();
      mark("device_settings");
    }),
  );
  q<HTMLButtonElement>("#ro-kv-add")?.addEventListener("click", () => kvList?.appendChild(kvRow()));

  const close = () => {
    document.removeEventListener("keydown", onKey);
    backdrop.remove();
    openEl = null;
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };
  document.addEventListener("keydown", onKey);
  backdrop.addEventListener("click", (e) => {
    if (e.target === backdrop) close();
  });
  q<HTMLButtonElement>("#ro-cancel")?.addEventListener("click", close);

  q<HTMLButtonElement>("#ro-apply")?.addEventListener("click", async () => {
    const tuner: Record<string, unknown> = {};

    if (changed.has("freq_correction_ppm"))
      tuner.freq_correction_ppm = num(q<HTMLInputElement>("#ro-ppm")?.value, 0);

    if (changed.has("gain")) {
      const agc = !!agcBox?.checked;
      tuner.gain_mode = agc ? "agc" : "manual";
      if (!agc) {
        const g: Record<string, number> = { ...(t.gain_elements_db ?? {}) };
        qa<HTMLElement>("#ro-gain .ro-slider").forEach((row) => {
          const name = row.dataset.el!;
          const s = row.querySelector<HTMLInputElement>('input[type="range"]');
          if (s) g[name] = parseFloat(s.value);
        });
        tuner.gain_elements_db = g;
      }
    }

    if (changed.has("bandwidth_hz"))
      tuner.bandwidth_hz = bwAuto?.checked ? null : num(bwInput?.value, 0);

    if (changed.has("sample_rate_hz"))
      tuner.sample_rate_hz = num(q<HTMLInputElement | HTMLSelectElement>("#ro-rate")?.value, t.sample_rate_hz);

    if (changed.has("antenna")) tuner.antenna = q<HTMLSelectElement>("#ro-ant")?.value ?? null;

    if (changed.has("dc_offset_correction"))
      tuner.dc_offset_correction = !!q<HTMLInputElement>("#ro-dc")?.checked;

    if (changed.has("device_settings")) {
      const map: Record<string, string> = {};
      qa<HTMLElement>("#ro-kv-list .ro-kv").forEach((row) => {
        const k = row.querySelector<HTMLInputElement>(".ro-kv-k")?.value.trim() ?? "";
        const v = row.querySelector<HTMLInputElement>(".ro-kv-v")?.value ?? "";
        if (k) map[k] = v;
      });
      tuner.device_settings = map;
    }

    if (Object.keys(tuner).length === 0) {
      close();
      return;
    }

    const btn = q<HTMLButtonElement>("#ro-apply")!;
    const msg = q<HTMLElement>("#ro-msg")!;
    btn.disabled = true;
    msg.textContent = "applying…";
    try {
      const updated = await client.patchRadio({ tuner });
      onApplied(updated);
      close();
    } catch (e) {
      btn.disabled = false;
      msg.textContent = `failed — ${(e as Error).message}`;
    }
  });
}
