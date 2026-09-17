---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Voice stub server

`voice-stub-server.py` speaks the Arbos voice protocol (`ios/Arbos/Voice/SelfHostedVoiceSession.swift` on `origin/cursor/ios-app-scaffold`) with scripted transcripts and a tone as reply audio. Use it to test the desktop client (PR `cursor/voice-linux-b027`) before the real server is up.

```
uv venv /tmp/voice-venv && uv pip install --python /tmp/voice-venv/bin/python websockets
/tmp/voice-venv/bin/python internal/voice/voice-stub-server.py --port 8790 --token secret
```

Desktop config (`~/.config/arbos/config.toml`):

```
voice_url = "ws://127.0.0.1:8790/voice"
voice_token = "secret"
```

Headless checks without a microphone or speaker: `ARBOS_VOICE_MIC_CMD='cat /tmp/mic.raw; sleep 60'` (any raw PCM16 24 kHz bytes; the stub emits one scripted word per 0.5 s of audio) and `ARBOS_VOICE_PLAYER_CMD='cat > /tmp/played.pcm'`.
