// FRS (Family Radio Service) channel plan — 22 fixed UHF channels, unlike
// the continuously-tunable AM/FM broadcast bands. No station database here:
// anyone can transmit on any channel from anywhere, so there's nothing to
// look up by location — just a fixed table straight from the FCC Part 95
// channel plan (channels 1-22, unified FRS/GMRS numbering since 2017).
//
// Receive-only for now: this lets the app monitor FRS traffic, not key up a
// transmission (see docs/architecture.md for why — HackRF isn't Part 95
// type-accepted equipment, so RF transmit on these channels is a separate,
// deliberately-gated feature, not implemented yet).

export interface FrsChannel {
  channel: number;
  freq_hz: number;
  /** FCC Part 95 FRS power limit on this channel, in watts. */
  max_power_w: number;
  /** Channels 1-7 and 15-22 are also usable by GMRS licensees (higher power,
   *  repeaters); 8-14 are FRS-exclusive, simplex-only, 0.5 W max. This is
   *  informational only here — it doesn't affect how the channel is tuned. */
  shared_gmrs: boolean;
}

// prettier-ignore
export const FRS_CHANNELS: FrsChannel[] = [
  { channel: 1,  freq_hz: 462_562_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 2,  freq_hz: 462_587_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 3,  freq_hz: 462_612_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 4,  freq_hz: 462_637_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 5,  freq_hz: 462_662_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 6,  freq_hz: 462_687_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 7,  freq_hz: 462_712_500, max_power_w: 2,   shared_gmrs: true },
  { channel: 8,  freq_hz: 467_562_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 9,  freq_hz: 467_587_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 10, freq_hz: 467_612_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 11, freq_hz: 467_637_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 12, freq_hz: 467_662_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 13, freq_hz: 467_687_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 14, freq_hz: 467_712_500, max_power_w: 0.5, shared_gmrs: false },
  { channel: 15, freq_hz: 462_550_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 16, freq_hz: 462_575_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 17, freq_hz: 462_600_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 18, freq_hz: 462_625_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 19, freq_hz: 462_650_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 20, freq_hz: 462_675_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 21, freq_hz: 462_700_000, max_power_w: 2,   shared_gmrs: true },
  { channel: 22, freq_hz: 462_725_000, max_power_w: 2,   shared_gmrs: true },
];

export const FRS_DEFAULT_FREQ_HZ = FRS_CHANNELS[0]!.freq_hz; // channel 1

export function frsChannelAt(freqHz: number): FrsChannel | undefined {
  return FRS_CHANNELS.find((c) => c.freq_hz === freqHz);
}

/** Step to the next/previous channel *by channel number*, not by frequency —
 *  channels 1-22 don't sit in ascending frequency order (15-22 interleave
 *  between 1-7 and 8-14), so arithmetic Hz stepping (like `stepAm`/`stepFm`)
 *  would visit them out of order. Falls back to channel 1 if the current
 *  frequency isn't a recognized FRS channel. */
export function stepFrsChannel(freqHz: number, dir: 1 | -1): number {
  const n = FRS_CHANNELS.length;
  const idx = FRS_CHANNELS.findIndex((c) => c.freq_hz === freqHz);
  const next = idx < 0 ? 0 : (idx + dir + n) % n;
  return FRS_CHANNELS[next]!.freq_hz;
}
