import { fetchSpeech } from "./api";
import { getSettings } from "./settings";

/* ------------------------------------------------------------------ */
/* Completion effects — what happens when an agent finishes a turn,     */
/* gated by the user's settings. The sound is a two-note WebAudio       */
/* chime (no asset to load); the notification only fires while the      */
/* window is unfocused — if you're watching the chat, you saw it end.   */
/* ------------------------------------------------------------------ */

let audioCtx: AudioContext | null = null;

function chime(): void {
  audioCtx ??= new AudioContext();
  const ctx = audioCtx;
  if (ctx.state === "suspended") void ctx.resume();
  const notes: [freq: number, at: number][] = [
    [880, 0],
    [1318.5, 0.1],
  ];
  for (const [freq, at] of notes) {
    const t = ctx.currentTime + at;
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.value = freq;
    gain.gain.setValueAtTime(0, t);
    gain.gain.linearRampToValueAtTime(0.08, t + 0.015);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.35);
    osc.connect(gain).connect(ctx.destination);
    osc.start(t);
    osc.stop(t + 0.4);
  }
}

/** Ask for notification permission; resolves true if granted. */
export async function ensureNotifyPermission(): Promise<boolean> {
  if (!("Notification" in window)) return false;
  if (Notification.permission === "granted") return true;
  if (Notification.permission === "denied") return false;
  return (await Notification.requestPermission()) === "granted";
}

/* ------------------------------------------------------------------ */
/* Spoken responses — the agent's reply read aloud on completion. One   */
/* utterance at a time: a new turn ending cuts off the previous one.    */
/* ------------------------------------------------------------------ */

/** How much of a reply gets spoken. A spoken summary substitutes for a
 *  glance at the screen, not for reading the whole answer. */
const SPEAK_MAX_CHARS = 600;

let speaking: { audio: HTMLAudioElement; url: string } | null = null;

/**
 * The speakable prose of a markdown reply: code blocks and images dropped,
 * link/emphasis/heading markup unwrapped, clamped to a few sentences.
 * Empty when the reply has no prose worth speaking.
 */
export function speakableText(markdown: string): string {
  let t = markdown
    .replace(/```[\s\S]*?```/g, " ") // fenced code (incl. charts)
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ") // images
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1") // links -> text
    .replace(/`([^`]*)`/g, "$1") // inline code -> content
    .replace(/^#{1,6}\s+/gm, "") // headings
    .replace(/(\*\*|__|\*|_|~~)/g, "") // emphasis markers
    .replace(/^\s*[-*+]\s+/gm, "") // list bullets
    .replace(/\s+/g, " ")
    .trim();
  if (t.length > SPEAK_MAX_CHARS) {
    const cut = t.slice(0, SPEAK_MAX_CHARS);
    const sentence = Math.max(cut.lastIndexOf(". "), cut.lastIndexOf("! "), cut.lastIndexOf("? "));
    t = sentence > SPEAK_MAX_CHARS / 3 ? cut.slice(0, sentence + 1) : cut;
  }
  return t;
}

/** Speak text via the gateway's TTS; resolves true once playback starts. */
async function speak(text: string): Promise<boolean> {
  const blob = await fetchSpeech(text);
  if (speaking) {
    // A newer turn ended: cut the previous utterance and release its blob.
    speaking.audio.pause();
    URL.revokeObjectURL(speaking.url);
  }
  const url = URL.createObjectURL(blob);
  const audio = new Audio(url);
  audio.onended = () => URL.revokeObjectURL(url);
  speaking = { audio, url };
  await audio.play();
  return true;
}

/**
 * An agent finished responding: speak / chime / notify, per the settings.
 * `finalText` is the reply's markdown; when spoken responses are on and it
 * has prose, the voice replaces the chime (one completion sound, not two).
 * TTS being unavailable (no audio endpoints, network) falls back to the
 * chime path so completion is never silent for a user who asked for sound.
 */
export function notifyRunComplete(title: string | null, finalText?: string): void {
  const settings = getSettings();
  const utterance = settings.speakResponses ? speakableText(finalText ?? "") : "";

  const chimePerSetting = () => {
    if (!settings.completionSound) return;
    try {
      chime();
    } catch {
      // No audio device / autoplay policy — nothing to do.
    }
  };

  if (utterance) {
    speak(utterance).catch(chimePerSetting);
  } else {
    chimePerSetting();
  }

  if (
    settings.systemNotifications &&
    !document.hasFocus() &&
    "Notification" in window &&
    Notification.permission === "granted"
  ) {
    new Notification(title || "Agent", { body: "Finished responding" });
  }
}
