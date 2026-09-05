// Audio over WebRTC: the browser normally offers recvonly and the server
// answers with a send-only Opus track. When a mic stream is supplied (FRS's
// PTT flow — see main.ts), the browser instead adds that track, which
// negotiates the connection as bidirectional (sendrecv) automatically —
// `addTrack` defaults to that direction, so no explicit transceiver
// juggling is needed. Non-trickle ICE either way (wait for gathering, then
// POST the offer once).

import type { Client } from "./api";

export type AudioState = "idle" | "connecting" | "playing" | "failed" | "closed";

export class AudioSession {
  readonly element: HTMLAudioElement;
  state: AudioState = "idle";
  onstate?: (state: AudioState, detail?: string) => void;

  private pc: RTCPeerConnection | null = null;

  constructor(
    private client: Client,
    private sessionId: string,
  ) {
    this.element = new Audio();
    this.element.autoplay = true;
    (this.element as HTMLAudioElement & { playsInline?: boolean }).playsInline = true;
  }

  /** Must be called from a user gesture so playback is allowed to start.
   *  `micStream`, when given, is added as an outgoing track — this is what
   *  actually makes the connection bidirectional; nothing elsewhere needs
   *  to know direction was negotiated one way or the other. */
  async start(micStream: MediaStream | null = null): Promise<void> {
    if (this.pc) return;
    this.set("connecting");

    const pc = new RTCPeerConnection({ iceServers: [] });
    this.pc = pc;
    const micTrack = micStream?.getAudioTracks()[0];
    if (micStream && micTrack) {
      pc.addTrack(micTrack, micStream);
    } else {
      pc.addTransceiver("audio", { direction: "recvonly" });
    }

    pc.ontrack = (e) => {
      this.element.srcObject = e.streams[0] ?? new MediaStream([e.track]);
      void this.element.play().catch(() => {});
    };
    pc.onconnectionstatechange = () => {
      switch (pc.connectionState) {
        case "connected":
          this.set("playing");
          break;
        case "failed":
          this.set("failed", "ICE / DTLS failed");
          break;
        case "disconnected":
        case "closed":
          if (this.state !== "closed") this.set("closed");
          break;
      }
    };

    try {
      await pc.setLocalDescription(await pc.createOffer());
      await gatheringComplete(pc);
      const offerSdp = pc.localDescription!.sdp;
      console.info(
        "lanline: offer candidates " +
          JSON.stringify(offerSdp.split("\n").filter((l) => l.includes("candidate")).map((l) => l.trim())),
      );
      console.info(
        "lanline: offer direction " +
          JSON.stringify(offerSdp.split("\n").filter((l) => /^a=(sendrecv|sendonly|recvonly|inactive)/.test(l.trim())).map((l) => l.trim())) +
          ` (senders=${pc.getSenders().length})`,
      );
      const answer = await this.client.audioOffer(this.sessionId, offerSdp);
      console.info(
        "lanline: answer candidates " +
          JSON.stringify(answer.sdp.split("\n").filter((l) => l.includes("candidate")).map((l) => l.trim())),
      );
      console.info(
        "lanline: answer direction " +
          JSON.stringify(answer.sdp.split("\n").filter((l) => /^a=(sendrecv|sendonly|recvonly|inactive)/.test(l.trim())).map((l) => l.trim())),
      );
      await pc.setRemoteDescription({ type: "answer", sdp: answer.sdp });
    } catch (e) {
      this.set("failed", (e as Error).message);
      this.teardown();
      throw e;
    }
  }

  async stop(): Promise<void> {
    this.teardown();
    try {
      await this.client.audioClose(this.sessionId);
    } catch {
      /* best effort */
    }
    this.set("closed");
  }

  setMuted(muted: boolean): void {
    this.element.muted = muted;
  }

  get muted(): boolean {
    return this.element.muted;
  }

  private teardown(): void {
    if (this.pc) {
      this.pc.ontrack = null;
      this.pc.onconnectionstatechange = null;
      this.pc.close();
      this.pc = null;
    }
    this.element.srcObject = null;
  }

  private set(state: AudioState, detail?: string): void {
    this.state = state;
    this.onstate?.(state, detail);
  }
}

function gatheringComplete(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      if (pc.iceGatheringState === "complete") {
        pc.removeEventListener("icegatheringstatechange", done);
        clearTimeout(timer);
        resolve();
      }
    };
    const timer = setTimeout(() => {
      pc.removeEventListener("icegatheringstatechange", done);
      resolve();
    }, 3000);
    pc.addEventListener("icegatheringstatechange", done);
  });
}
