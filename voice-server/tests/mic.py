"""A microphone for a machine without one: `ARBOS_VOICE_MIC_CMD="python -m tests.mic <dir>"`.

Writes raw PCM16 mono 24 kHz to stdout at real time: silence, and whenever a `*.raw` file
appears in `<dir>` its samples (oldest first), then silence again. The harness drops utterance
files there to make the desktop "speak". The file is removed once it has been played.
"""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path

RATE = 24_000
FRAME_MS = 100
FRAME = RATE * FRAME_MS // 1000 * 2


def main() -> None:
    d = Path(sys.argv[1])
    d.mkdir(parents=True, exist_ok=True)
    out = sys.stdout.buffer
    silence = bytes(FRAME)
    pending = b""
    next_at = time.monotonic()
    while True:
        if not pending:
            files = sorted(d.glob("*.raw"))
            if files:
                pending = files[0].read_bytes()
                try:
                    files[0].unlink()
                except OSError:
                    pass
        frame, pending = (pending[:FRAME], pending[FRAME:]) if pending else (silence, b"")
        if len(frame) < FRAME:
            frame = frame + bytes(FRAME - len(frame))
        try:
            out.write(frame)
            out.flush()
        except BrokenPipeError:
            return
        next_at += FRAME_MS / 1000
        delay = next_at - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        else:
            next_at = time.monotonic()


if __name__ == "__main__":
    main()
