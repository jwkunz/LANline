# Legal

**This is not legal advice.** It is a plain-language summary of the rules that
apply to running an SDR, written for the United States, current as of 2026.
Regulations differ by country and change over time. **You — the operator —
are solely responsible for how you use your radio hardware and the spectrum.**

---

## License

LANline is released under the **MIT License** — see [`../LICENSE`](../LICENSE).

> Copyright © 2026 Numerius Engineering LLC.
> Author: Jake Kunzler.

The MIT license lets you use, copy, modify and redistribute the software
freely, including commercially, provided the copyright notice and permission
notice are kept. The software is provided **"AS IS", without warranty of any
kind**; the copyright holder is not liable for any claim or damages arising
from its use.

### Third-party components

| Component | Role | License |
|---|---|---|
| `candle` (candle-core / -nn / -transformers) | pure-Rust Whisper for STT | MIT / Apache-2.0 |
| `tokenizers` (Hugging Face) | Whisper tokenizer | Apache-2.0 |
| `webrtc-rs` | WebRTC audio transport | MIT / Apache-2.0 |
| `rustls` + `ring` | TLS for the C2 port and outbound HTTPS | ISC / MIT / OpenSSL-style (`ring`) |
| `reqwest` | outbound HTTPS (flight / vessel lookup only) | MIT / Apache-2.0 |
| `rustfft`, `num-complex`, `bytes`, `axum`, `tokio`, `serde`, … | server plumbing / DSP | MIT / Apache-2.0 |
| `mdns-sd` | `_lanline._tcp` service discovery | MIT / Apache-2.0 |
| `satellite.js` (web client) | in-browser SGP4 pass prediction for NOAA APT | MIT |
| **OpenAI Whisper model weights** (`whisper-base.en`, downloaded by you) | STT model | MIT (weights); you download them yourself |
| **espeak-ng** (optional, `--tts`) | text-to-speech backend | **GPL-3.0** — invoked as a **separate subprocess**, not linked into LANline; installing it is your choice |
| **Piper** (optional, `--tts-voice`) | neural text-to-speech backend | MIT (binary); voice models vary — check each |
| SoapySDR + device plugins (radioconda) | hardware abstraction | Boost / LGPL per module |

`espeak-ng` is GPLv3. LANline does **not** link it — the `tts` feature shells
out to the `espeak-ng` binary if you have installed it. That keeps LANline's
MIT license intact; your installation of espeak-ng is governed by its own
license. If that distinction matters for your redistribution, use Piper or
ship without the `tts` feature.

---

## Receiving

In the United States it is **generally lawful to receive** radio
transmissions. Every mode LANline decodes is a signal **intended for open
reception**:

- **NOAA Weather Radio**, **FM / AM broadcast** — public broadcast services.
- **Aircraft ADS-B** (1090 MHz) — an unencrypted position-broadcast standard.
- **Marine AIS** (161.975 / 162.025 MHz) — an unencrypted safety broadcast.
- **Amateur (ham) FM**, **APRS** (144.390 MHz) — amateur radio, which is
  public by rule (Part 97 forbids encryption of the message content).
- **NOAA APT** (137 MHz) — a civilian weather-satellite image downlink.

LANline does **not** target, tune, or decode cellular, cordless-phone,
common-carrier, public-safety trunked, or encrypted traffic.

Two federal statutes still bound what you may do with what you hear:

- **Communications Act § 705 (47 U.S.C. § 605)** and the **Electronic
  Communications Privacy Act (18 U.S.C. § 2511)** prohibit the **interception
  and divulgence or use** of certain private communications — notably
  cellular/PCS, common-carrier traffic, and any scrambled or encrypted
  communication — even when your hardware can technically receive them.
- Some states restrict use of a "mobile scanner" in a vehicle.

Receiving a broadcast that is meant for the public — which is all LANline
does — is not the conduct those statutes target. Recording, transcribing
(the STT feature), or re-transmitting **private** communications could be.
Don't.

---

## Transmitting

**Transmit is disabled by default.** LANline never keys a transmitter unless
you start it with `--enable-tx` (or `LANLINE_ENABLE_TX=true`) **and** select a
transmit-capable device. With transmit off, LANline is a receive-only
appliance.

If you do enable it, understand what you are turning on:

### A general-purpose SDR is not FCC-certified equipment

A HackRF, PlutoSDR, LimeSDR, etc. is a laboratory / experimental instrument.
It has **no FCC equipment authorization** for any licensed radio service, and
a bare SDR has minimal output filtering — its transmitted signal can carry
harmonics and spurious emissions well outside the intended channel.

### FRS / GMRS (FCC Part 95)

The Family Radio Service and GMRS are **certified-equipment** services: only
radios that hold an FCC Part 95 grant may transmit on them, regardless of
power level or your intent. **An SDR is not, and cannot be, Part 95
type-accepted.**

LANline's FRS transmit path exists because the project owner asked for it, for
their own informed, non-distributed experimentation on their own equipment.
**It is not presented as compliant**, and this project does not encourage
transmitting on FRS/GMRS with an SDR.

### Amateur radio (FCC Part 97)

Part 97 **permits home-built and non-certified equipment** operated by a
**licensed control operator** on amateur frequencies (§ 97.5, § 97.109). This
is the legitimate use of LANline's transmit feature. The APRS transmit path
(144.390 MHz) is amateur operation.

As the control operator you remain responsible for:

- holding a valid amateur license of the appropriate class for the
  frequency, mode and power;
- **staying within the amateur band edges** and using an appropriate mode for
  the sub-band (LANline's ham picker shows the voluntary ARRL band plan, but
  it does not enforce it);
- **spurious-emission limits** (§ 97.307) — in practice this means putting a
  **band-pass or low-pass filter** between the SDR and the antenna;
- **minimum necessary power** (§ 97.313);
- **station identification** (§ 97.119) — LANline does not ID for you;
- not causing harmful interference, and ceasing on notice.

### Outside the United States

Everything above is FCC-specific. Many countries require an individual
license for **any** transmission from an SDR; some prohibit SDR transmission
entirely; amateur reciprocal-operating rules vary. Consult your national
regulator (Ofcom, ISED, ACMA, BNetzA, …) before transmitting.

---

## Internet access

LANline is designed to run on an isolated LAN. **One feature reaches the
public internet:** the ADS-B / AIS enrichment lookup.

- When you tap an aircraft, the server sends its **ICAO 24-bit address** (and
  decoded callsign) to **adsbdb.com**. When you tap a vessel, it sends the
  **MMSI** to **vesselfinder.com**. Nothing else about your location or your
  receiver is transmitted.
- Results are cached on the server and shared across clients.
- Turn it off entirely with **`--flight-lookup false`** or
  **`LANLINE_FLIGHT_LOOKUP=0`** — the feature and its `capabilities` flags
  disappear and no outbound request is ever made.

The station / repeater / satellite databases the web client uses (FM, AM,
NWR, ham repeaters, APT orbital elements) are **bundled into the build** by
the maintainer's `scripts/fetch-*.mjs` — the running server and clients do
not contact those sources.

---

## Third-party data attribution

| Data | Source | Notes |
|---|---|---|
| FM / AM / NOAA-WX station locations | U.S. FCC LMS / CDBS public databases | public-domain U.S. government data |
| Amateur repeater directory | [hearham.com](https://hearham.com) open repeater data | bundled snapshot; regenerate with `scripts/fetch-repeaters.mjs` |
| Aircraft registration / type / route | [adsbdb.com](https://www.adsbdb.com) | community API; used live only for flight lookup; see their terms |
| Vessel particulars / photo | [vesselfinder.com](https://www.vesselfinder.com) | used live only for vessel lookup; see their terms |
| Satellite orbital elements (TLE) | CelesTrak / NOAA | bundled snapshot (`apt-tle.json`); ~1–2 week accuracy |

Aircraft/vessel data is provided by those third parties under their own terms
of use and with no guarantee of accuracy. Do not rely on any of it for
navigation, safety, or operational decisions.

---

## Summary

- **Receiving** the broadcast services LANline decodes: fine in the U.S.
  Don't record or divulge anything private.
- **Transmitting**: off unless you turn it on. For **licensed amateur use**
  it is legitimate homebrew operation and you carry the control-operator
  responsibilities above (band edges, filtering, power, ID). For **FRS/GMRS**
  an SDR is not legal equipment — this project does not present that path as
  compliant.
- **Privacy**: the only outbound internet traffic is the opt-out
  flight/vessel lookup.
- **No warranty. Not legal advice. Your radio, your responsibility.**
