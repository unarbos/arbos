/* ------------------------------------------------------------------ */
/* Browser microphone recorder — the mic button's capture path when     */
/* the host has no on-device dictation (Apple Speech is macOS-only).    */
/* MediaRecorder captures a compressed clip; the gateway's              */
/* /api/voice/transcribe turns it into text. One recording at a time —  */
/* there is one mic button.                                             */
/* ------------------------------------------------------------------ */

/** MediaRecorder mime → the transcription endpoint's format label. */
const MIME_FORMATS: [mime: string, format: string][] = [
  ["audio/webm", "webm"],
  ["audio/mp4", "m4a"], // Safari
  ["audio/ogg", "ogg"],
];

interface ActiveRecording {
  rec: MediaRecorder;
  stream: MediaStream;
  chunks: Blob[];
  format: string;
}

let active: ActiveRecording | null = null;

/** Whether this browser can capture audio at all. */
export function recorderSupported(): boolean {
  return (
    typeof MediaRecorder !== "undefined" &&
    !!navigator.mediaDevices?.getUserMedia &&
    MIME_FORMATS.some(([m]) => MediaRecorder.isTypeSupported(m))
  );
}

/** Start capturing from the browser's microphone (prompts for permission). */
export async function startRecording(): Promise<void> {
  if (active) throw new Error("a recording is already in progress");
  const picked = MIME_FORMATS.find(([m]) => MediaRecorder.isTypeSupported(m));
  if (!picked) throw new Error("this browser cannot record audio");
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const rec = new MediaRecorder(stream, { mimeType: picked[0] });
  const entry: ActiveRecording = { rec, stream, chunks: [], format: picked[1] };
  rec.ondataavailable = (e) => {
    if (e.data.size > 0) entry.chunks.push(e.data);
  };
  rec.start();
  active = entry;
}

/** Stop capturing and return the clip as base64 plus its container format. */
export async function stopRecording(): Promise<{ data: string; format: string }> {
  const entry = active;
  active = null;
  if (!entry) throw new Error("no recording in progress");
  const stopped = new Promise<void>((resolve) => {
    entry.rec.onstop = () => resolve();
  });
  entry.rec.stop();
  await stopped;
  entry.stream.getTracks().forEach((t) => t.stop());
  const blob = new Blob(entry.chunks, { type: entry.rec.mimeType });
  return { data: await blobBase64(blob), format: entry.format };
}

/** Abandon an in-flight recording without transcribing (error paths). */
export function cancelRecording(): void {
  const entry = active;
  active = null;
  if (!entry) return;
  try {
    entry.rec.stop();
  } catch {
    // Already stopped.
  }
  entry.stream.getTracks().forEach((t) => t.stop());
}

/** A Blob's bytes as bare base64 (no data: prefix). */
export function blobBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      resolve(url.slice(url.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(blob);
  });
}
