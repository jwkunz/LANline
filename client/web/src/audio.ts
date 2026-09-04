// Receive-audio over WebRTC: the browser offers recvonly, the server answers
// with a send-only Opus track. Non-trickle ICE (wait for gathering, then POST
// the offer once).

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

  /** Must be called from a user gesture so playback is allowed to start. */
  async start(): Promise<void> {
    if (this.pc) return;
    this.set("connecting");

    const pc = new RTCPeerConnection({ iceServers: [] });
    this.pc = pc;
    pc.addTransceiver("audio", { direction: "recvonly" });

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
        "sdrc2: offer candidates " +
          JSON.stringify(offerSdp.split("\n").filter((l) => l.includes("candidate")).map((l) => l.trim())),
      );
      const answer = await this.client.audioOffer(this.sessionId, offerSdp);
      console.info(
        "sdrc2: answer candidates " +
          JSON.stringify(answer.sdp.split("\n").filter((l) => l.includes("candidate")).map((l) => l.trim())),
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
