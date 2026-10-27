import { Channel, invoke } from "@tauri-apps/api/core";

/** Mirrors `speech::SpeechEvent` on the Rust side. */
type SpeechEvent =
  | { kind: "engine"; name: string; fallback: boolean }
  | { kind: "clip"; mime: string; audio: string }
  | { kind: "done" }
  | { kind: "error"; message: string };

export type PlayerState =
  | { phase: "idle" }
  | { phase: "loading"; engine: string; fallback: boolean }
  | { phase: "playing"; engine: string; fallback: boolean }
  | { phase: "error"; message: string };

/** A beat of silence between clips, so sentences do not run into each other. */
const GAP_SECONDS = 0.12;

/**
 * Plays what Rust synthesises, clip by clip, as it arrives. Clips are decoded and
 * scheduled back to back on one AudioContext, so the joins are seamless rather than
 * waiting on each `<audio>` element to finish and the next to start.
 */
export class SpeechPlayer {
  private context: AudioContext | null = null;
  private sources = new Set<AudioBufferSourceNode>();
  private nextStart = 0;
  private session = 0;
  private finished = false;
  private engine = "";
  private fallback = false;

  constructor(private readonly onState: (state: PlayerState) => void) {}

  get active(): boolean {
    return this.sources.size > 0 || !this.finished;
  }

  /** `preview` reads with exactly that engine and voice, with no fallback. */
  async play(text: string, lang: string | null, preview?: { engine: string; voice: string }) {
    this.halt();
    const session = ++this.session;
    this.finished = false;
    this.engine = "";
    this.fallback = false;

    // Created on the click that started playback, which is what autoplay rules want.
    this.context ??= new AudioContext();
    await this.context.resume();
    this.nextStart = 0;

    // Decoding is asynchronous; chaining keeps clips in the order they were sent.
    let queue = Promise.resolve();
    const channel = new Channel<SpeechEvent>();
    channel.onmessage = (event) => {
      if (session !== this.session) return;
      queue = queue.then(() => this.handle(session, event)).catch(console.error);
    };

    this.onState({ phase: "loading", engine: "", fallback: false });
    try {
      await invoke("speak", { text, lang, preview: preview ?? null, onEvent: channel });
    } catch (e) {
      if (session !== this.session) return;
      this.finished = true;
      this.onState({ phase: "error", message: String(e) });
    }
  }

  stop() {
    this.halt();
    this.finished = true;
    void invoke("stop_speaking");
    this.onState({ phase: "idle" });
  }

  private halt() {
    this.session++;
    for (const source of this.sources) {
      source.onended = null;
      try {
        source.stop();
      } catch {
        // Already stopped.
      }
    }
    this.sources.clear();
  }

  private async handle(session: number, event: SpeechEvent) {
    switch (event.kind) {
      case "engine":
        this.engine = event.name;
        this.fallback = event.fallback;
        if (this.sources.size === 0) {
          this.onState({ phase: "loading", engine: this.engine, fallback: this.fallback });
        }
        return;
      case "clip":
        await this.schedule(session, event.audio);
        return;
      case "done":
        this.finished = true;
        if (this.sources.size === 0) this.onState({ phase: "idle" });
        return;
      case "error":
        this.finished = true;
        // Whatever already played stays played; only report if nothing is still going.
        if (this.sources.size === 0) this.onState({ phase: "error", message: event.message });
        return;
    }
  }

  private async schedule(session: number, base64: string) {
    const context = this.context;
    if (!context) return;
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const buffer = await context.decodeAudioData(bytes.buffer);
    if (session !== this.session) return;

    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(context.destination);
    const at = Math.max(context.currentTime + 0.03, this.nextStart);
    source.start(at);
    this.nextStart = at + buffer.duration + GAP_SECONDS;
    this.sources.add(source);
    this.onState({ phase: "playing", engine: this.engine, fallback: this.fallback });

    source.onended = () => {
      this.sources.delete(source);
      if (session === this.session && this.finished && this.sources.size === 0) {
        this.onState({ phase: "idle" });
      }
    };
  }
}
