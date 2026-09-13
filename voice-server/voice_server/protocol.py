"""Wire protocol shared with the Arbos iOS client (`SelfHostedVoiceSession.swift`).

One WebSocket per call. Binary frames carry raw PCM16 little-endian mono
audio in both directions (24 kHz unless `session.start` says otherwise).
Text frames carry JSON objects tagged with `type`.
"""

DEFAULT_RATE = 24_000
ASR_RATE = 16_000

# client -> server
SESSION_START = "session.start"
SPEAK = "speak"
INTERRUPT = "interrupt"
SESSION_END = "session.end"

# server -> client
SESSION_READY = "session.ready"
SPEECH_STARTED = "speech.started"
SPEECH_STOPPED = "speech.stopped"
TRANSCRIPT_DELTA = "transcript.delta"
TRANSCRIPT_FINAL = "transcript.final"
RESPONSE_STARTED = "response.started"
RESPONSE_TRANSCRIPT = "response.transcript"
RESPONSE_DONE = "response.done"
ERROR = "error"

PROTOCOL_TEXT = """\
WIRE PROTOCOL (matches ios/Arbos/Voice/SelfHostedVoiceSession.swift)

  URL      wss://HOST/ws?token=TOKEN        (also accepted: any path except /healthz,
                                             or header  Authorization: Bearer TOKEN)
  Audio    binary WebSocket frames, raw PCM16 little-endian mono, 24 kHz both ways.
           Any frame size works; 20-100 ms per frame is typical.
  Control  text frames, one JSON object each, tagged with "type".

  client -> server
    {"type":"session.start","format":{"type":"audio/pcm","rate":24000}}
        Optional. Sets the sample rate for both directions (8000-48000).
        Audio sent before it is treated as 24 kHz. Optional extra keys:
          "language": "en"  ASR language hint (default from --language)
          "voice": "af_heart"  TTS voice for this session (default from --voice)
          "reply": "none" | "openrouter"  who answers the user (default from --reply)
    <binary>                       microphone audio
    {"type":"speak","text":"..."}  voice this text; requests queue in order
    {"type":"interrupt"}           drop the current reply and everything queued
    {"type":"session.end"}         close

  server -> client
    {"type":"session.ready","rate":24000,"asr":"...","tts":"...","reply":"none"}
    {"type":"speech.started"}      server VAD heard the user start talking. If a
                                   reply was playing it is cancelled at the same
                                   moment (barge-in) and response.done follows.
    {"type":"speech.stopped"}      end of the utterance (after --end-silence-ms)
    {"type":"transcript.delta","text":" more words"}
                                   APPEND to the open user line (increment only)
    {"type":"transcript.final","text":"Whole cleaned-up utterance."}
                                   REPLACES the open user line and closes it.
                                   May be "" when the audio held no words.
    {"type":"response.started"}    only when the server answers on its own
                                   (--reply openrouter); never for "speak"
    <binary>                       reply audio, sent as fast as it is synthesised;
                                   the client buffers and plays it
    {"type":"response.transcript","text":"..."}
                                   what the audio says; only for server-originated
                                   replies (for "speak" the client already has the text)
    {"type":"response.done"}       end of one reply. Adds "interrupted":true when it
                                   ended because of interrupt or barge-in.
    {"type":"error","message":"..."}

  Turn shape
    speech-only (default):  audio -> speech.started -> transcript.delta* ->
        speech.stopped -> transcript.final. The client sends the text to the
        Arbos kernel and returns the reply with "speak" -> <binary>* -> response.done.
    server reply (--reply openrouter):  ... -> transcript.final -> response.started
        -> (response.transcript, <binary>*)* -> response.done.

  Barge-in
    The server cancels its own output the moment it hears the user. Audio already
    sent cannot be recalled, so the client must flush its playback queue on
    speech.started (the iOS client does). The client may also send "interrupt".

  Health   GET /healthz -> 200 "ok" (no auth)
"""
