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
TEXT_INPUT = "text.input"
TEXT_CANCEL = "text.cancel"
CLIENT_SPEAKING = "client.speaking"
SESSION_END = "session.end"

# server -> client, text channel and agent bridge
TEXT_DELTA = "text.delta"
TEXT_DONE = "text.done"
TOOL_CALL = "tool.call"
TOOL_RESULT = "tool.result"
AGENT_EVENT = "agent.event"
AGENT_TURN = "agent.turn"
AGENT_TREE = "agent.tree"
AGENT_DONE = "agent.done"
AGENT_ACTIVITY = "agent.activity"
NARRATOR_SAY = "narrator.say"

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
          "instructions": "..."  system prompt for the speech model (duplex engine)
          "answerer": "auto"|"kernel"|"model"  duplex call mode: who answers a spoken turn (see Engines)
          "project": {"machine":"arboslife","project":"demo","path":"/Users/j/demo","host":null,
                      "name":"demo","context":{...}}   SCOPE THE CALL to the folder the client has
                open. The path decides: the server's own kernel when it serves that very folder;
                else, for a folder on the server's machine with a live kernel, a direct attach to
                it; else the hub (--hub), matched by the roster's `place` when known, by
                machine/project otherwise. Also accepted: "project":"arboslife/demo",
                "kernel":"arboslife/demo" or an "arbos://…/" store address (name only; no path).
                If the folder has no running kernel, is not on the roster, or does not answer, the
                server sends {"type":"error","code":"project_unknown"|"project_offline"|
                "project_unreachable"|"no_hub","project":"machine/project","message":…} and
                closes the socket with code 4404. A dict without "machine" (the client is off the
                hub) whose path is not a folder on the server's machine gets "project_not_on_hub"
                with a message saying how to register. It never answers from another project.
                "context" (optional): what the client shows when the call starts, so the narrator
                and the speech model know the chat: {"recent":[{"role":"user"|"assistant"|"worker"|
                "tool"|"notice"|"asked"|"thinking","text":"..."}], "agents":[{"name","state","step"}],
                "running":bool}. Tool lines are labels only (never a diff or a file body).
          "agents": true|false  mirror kernel events (agent.*) to this client (default on when a kernel is attached)
          "mode": "call"  CALL MODE (see below): talk to a project's main agent; the narrator speaks highlights
          "project": "<machine>/<project>"  which project the call is for. A hub name, when the gateway has --hub:
                                            the call attaches to that kernel through the hub (session.ready says
                                            "via":"hub"). Empty, or the gateway's own project: its kernel ("via":"gateway").
                                            A bare folder name the gateway does not serve ("discord_backups" from an
                                            older client on a machine that is not on the hub) is REFUSED the same way:
                                            error {"code":"project_not_on_hub","project":"discord_backups","message":...}
                                            then close 4404. The call never lands on another kernel.
          "channel": "voice"  what the caller's utterances are filed as in the agent's inbox (voice | text)
          "device": "desktop"  which client this is (phone | desktop); written beside channel on every message
          "screen": "on your screen"  how the narrator refers to the client's display ("in the chat" on a phone)
    <binary>                       microphone audio
    {"type":"speak","text":"..."}  voice this text; requests queue in order
    {"type":"interrupt"}           drop the current reply and everything queued
    {"type":"text.input","text":"..."}   TEXT CHANNEL: a typed turn; answered with text.delta* + text.done
    {"type":"text.cancel"}         stop the running text turn
    {"type":"client.speaking","speaking":true|false,"route":"speaker"|"airpods"|"headset"|...}
                                   optional: the app is playing reply audio right now. Tightens the
                                   server's echo gate (see below); a headset route turns the gate off
                                   (those cancel their own echo). Send false when playback drains.
    {"type":"session.end"}         close

  server -> client
    {"type":"session.ready","rate":24000,"engine":"duplex"|"pipeline","asr":"...","tts":"...",
     "reply":"...","text":"...","tools":["send_agent","agent_status","ask_arbos"],"kernel":true,
     "answerer":"auto","project":"arboslife/demo"|"","via":"own"|"local"|"hub"|"gateway",
     "project_path":"/home/const/arbos-hub/projects/demo",
     "project_info":{"machine":"arboslife","project":"demo","name":"demo","icon":"folder",
     "store":"arbos://arboslife/demo/","kind":"project","path":"/home/const/arbos-hub/projects/demo",
     "place":"/home/const/arbos-hub/projects/demo","via":"hub"}
       | {...,"kind":"gateway","via":"gateway","place":"/srv/x"|null,"url":"tcp://..."|null} | null}
                                   project is the label the call was scoped to ("" = none named). project_path
                                   is the folder the call is bound to: a client compares it with the folder it
                                   asked for and hangs up on a mismatch. project_info describes the kernel on
                                   the line: from the hub roster or the local folder when the call named a
                                   project ("path" = "place" = the folder it serves = the call's working
                                   directory; via own|local|hub says how it was reached); the gateway's own
                                   kernel, named as such (kind gateway), when none was named; null when the
                                   gateway has no kernel. With --engine openai the same brief goes to GPT-Live
                                   as instructions (see CALL MODE below).
    {"type":"speech.started"}      server VAD heard the user start talking. If a
                                   reply was playing it is cancelled at the same
                                   moment (barge-in) and response.done follows.
    {"type":"speech.stopped"}      end of the utterance (after --end-silence-ms)
    {"type":"transcript.delta","text":" more words"}
                                   APPEND to the open user line (increment only)
    {"type":"transcript.final","text":"Whole cleaned-up utterance."}
                                   REPLACES the open user line and closes it.
                                   May be "" when the audio held no words.
    {"type":"response.started","speaker":"narrator"?}
                                   only when the server answers on its own
                                   (--reply openrouter); never for "speak". In call mode
                                   speaker="narrator" marks the narrator's lines (already sent
                                   as narrator.say); absent = the speech model's own words.
    <binary>                       reply audio, sent as fast as it is synthesised;
                                   the client buffers and plays it
    {"type":"response.transcript","text":"..."}
                                   what the audio says; only for server-originated
                                   replies (for "speak" the client already has the text)
    {"type":"response.done","reason":"completed"|"interrupted"|"superseded"|"failed"}
                                   end of one reply. "interrupted" (also "interrupted":true, kept
                                   for old clients): the user talked over it. "superseded": more
                                   of the question arrived before the answer played; treat it as
                                   nothing having happened. A superseded answer that never produced
                                   audio sends no response.done at all (it was never announced).
    {"type":"error","message":"..."}

    text channel
    {"type":"text.delta","text":"..."}   streamed answer to text.input
    {"type":"text.done","text":"<whole answer>","cancelled":false}

    agent bridge (only when the server is attached to an Arbos kernel)
    {"type":"tool.call","name":"send_agent","arguments":{"task":"..."}}   the voice model acted
    {"type":"tool.result","name":"send_agent","output":"..."}
    {"type":"agent.done","agent":"<id>","text":"<report>"}   a dispatched agent finished; the report
                                   is also spoken (gateway voice) as a normal reply turn
    {"type":"agent.event","agent":"root","kind":"assistant"|"assistant_final"|"say"|"user"|"tool"|"notice","text":"..."}
                                   mirror of the kernel's transcript so the app can show the main chat;
                                   "assistant" = streamed increment, "assistant_final" = the whole reply
                                   once the turn ends (replace the streamed line with it)
    {"type":"agent.turn","agent":"root","state":"running"|"idle"}
    {"type":"agent.tree","agents":[{"id","name","parent"}]}

    call mode (session.start {"mode":"call"}; session.ready then has "mode":"call","narrator":true)
    {"type":"agent.activity","agent":"root","state":"working"|"tool"|"idle","tool":"bash","detail":"cargo build",
     "since_ms":4200,"heartbeat":false}
                                   what the kernel is doing, from its own turn and tool frames: one frame per
                                   transition for every agent of the call (the main agent and its workers), and
                                   the current state again every 5 s with heartbeat:true while any is not idle.
                                   Play the sound of work on this and nothing else; when the beats stop, the
                                   state is unknown. session.ready says "activity":true when these come.
    {"type":"narrator.say","text":"...","kind":"highlight"|"report"|"ask"|"error"|"detail","ref":"transcript:1181"}
                                   the narrator is about to voice this line (as a normal reply turn:
                                   response.started, response.transcript, audio, response.done). Write it
                                   into the chat as a `voice ·` line so the record is complete.
    Every finished caller utterance is sent to the project's main agent as a user message with
    channel = "voice" (the kernel files it in the agent's inbox with that key; a running turn takes
    it as a steer). text.input during a call goes the same way with channel = "text" and is answered
    with text.done {"text":"","forwarded":true}. The narrator speaks: the main agent's reply at the end
    of each turn (first sentence plus the one naming a result, at most 240 characters, "The rest is on
    your screen." when anything was cut), a sub-agent finishing ("<name> is done: <last words>"),
    questions (ask frames; the caller's next utterance answers them), and failures. It never reads tool
    output, diffs, lists or code. The speech model's tools in call mode are more_detail(question) —
    answered from the transcript through the kernel's tail/read frames, no new work — and
    agent_status; it does not dispatch work itself (the words already went to the main agent).

  Engines
    duplex (default when the model is up): NVIDIA NemotronLabs VoiceChat 11B, one full-duplex
        speech-to-speech model. No turn-taking: it listens while it talks, yields when the
        user cuts in, and calls the Arbos tools mid-conversation. Events above are produced by
        the model. "speak" is voiced by the gateway TTS (the model cannot be told what to say).
        Call mode (--answerer, default auto): when a kernel is attached, every spoken turn that
        is not small talk goes to the kernel's main agent as a text turn and its reply is voiced
        by the gateway TTS (response.started / response.transcript / audio / response.done); the
        model's own reply for that turn is dropped and its tool calls are answered "already
        handled". Small talk (greetings, thanks, "can you hear me") is left to the model. The
        model still hears the user, so barge-in over a kernel answer works the same way.
    openai (GPT-Live, --engine openai, env OPENAI_API_KEY): OpenAI's full-duplex model handles
        listening, speaking and *deciding when to hand off*; the Arbos kernel is the backend
        (client delegation). Small talk is answered by the model at once; anything about the
        user's work is delegated: the gateway speaks "Yeah, one sec." once the kernel is
        actually running (not on a timer; the model's own "let me check" is muted while the
        kernel works). The kernel's answer is appended as commentary and spoken unprompted
        when it arrives, even after a barge-in; barge-ins duck the working line and never
        cancel kernel work (a new question does, via stop). Approvals and asks
        are still ours: spoken in the model's voice, answered by the caller's yes or no. Our VAD +
        Whisper still produce transcript.final. Billing: $0.05 per minute of session, per second,
        plus the kernel's own model calls.
        What GPT-Live is told about the project (all from the call's kernel and the caller's
        on-screen snapshot, never the gateway's own folder): at session start, a brief in its
        instructions with three stores — PROJECT IDENTITY (name, machine/project, the folder the
        kernel serves = the working directory, the arbos:// address; or, when the call named no
        project, that the backend is the gateway's default kernel and where that is), ON-SCREEN
        CHAT (what the client is showing: user, Arbos, workers, tool labels, notices, asks), and
        WORKERS AND ACTIVITY (the client's sub-agent list plus the kernel's live status). The last
        VOICE_LIVE_HISTORY (40) visible lines of that chat go in as startup history (session.input),
        on-screen first, older kernel lines filling the rest; tool lines are names only, never
        diffs or file bodies. During the call, quiet context (session.thinking.append, coalesced
        every 3 s): lines typed in the project chat, Arbos's text replies it did not relay, worker
        reports, the main agent and workers starting/finishing, and the tools they run (name +
        short detail). Delegated utterances reach the kernel as `user` lines with channel "voice"
        and the caller's device. Typed text.input during a GPT-Live call goes to the kernel (steer
        while a turn runs) unless an ask is open, which it answers.
        Voice rows in a client's chat are DISPLAY ONLY: draw transcript.final as the caller's line
        and response.transcript (accumulated to response.done) as Arbos's spoken line; the kernel's
        own `user` event with channel "voice" for the same words is that same line, not a new one.
        Never send those words to the kernel again.
    pipeline (fallback, any GPU or CPU): Silero VAD -> faster-whisper -> optional reply hop
        (OpenRouter with the same tools, or the kernel) -> Kokoro. Explicit turns; barge-in is
        server-side cancellation on speech.started.

  Turn shape (pipeline)
    speech-only (--reply none):  audio -> speech.started -> transcript.delta* ->
        speech.stopped -> transcript.final. The client sends the text to the
        Arbos kernel and returns the reply with "speak" -> <binary>* -> response.done.
    server reply (--reply openrouter|kernel):  ... -> transcript.final -> response.started
        -> (response.transcript, <binary>*)* -> response.done.

  Acting ("send an agent to ...")
    The voice model calls send_agent(task). The gateway asks the kernel's main agent to spawn
    a sub-agent (kernel attach protocol, `user` frame) and returns at once. When the sub-agent
    goes idle, its last `say` is sent as agent.done and spoken. agent_status and ask_arbos
    answer synchronously (ask_arbos waits up to 45 s for the main agent).

  Barge-in
    Audio already sent cannot be recalled, so the client must flush its playback queue on
    speech.started (the iOS client does). The client may also send "interrupt".

  Output level
    Reply audio from both engines is peak-normalised toward --out-target-dbfs (default -3 dBFS)
    with a soft-knee limiter: gain drops at once on loud input, rises 6 dB/s on quiet input, up
    to +24 dB, and holds over silence. --no-normalize sends the engine's raw level.

  Echo gate (server side, independent of the phone's echo cancellation)
    While reply audio is on its way to the speaker (and 0.6 s after), the last 160 ms of
    uplink is cross-correlated with the reply audio sent in the last 3 s. The gate closes
    only after an echo path is confirmed (three close matches); until then, and with a
    headset route, everything passes. A confirmed echo is replaced with silence before the
    speech model or VAD sees it; uplink louder than the predicted echo (--echo-margin) is
    the user talking over us and passes, so barge-in keeps working. "client.speaking":true
    lowers the thresholds. --no-echo-gate turns it off.

  Health   GET /healthz -> 200 "ok" (no auth)
"""
