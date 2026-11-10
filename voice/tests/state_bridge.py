"""Bounded JSONL observations of the actual pure voice state machines.

Run with python3 -I -B. No SDK, application configuration or effect adapters are
imported. Token references select attempts actually issued earlier in the trace.
"""
from dataclasses import fields, is_dataclass
import json
from pathlib import Path
import sys

MAX_LINE = 65_536
MAX_EVENTS = 64


def guard(event, args):
    if event.startswith("socket.") or event in {"subprocess.Popen", "os.system"}:
        raise RuntimeError("External access is forbidden")
    if event == "open" and args and isinstance(args[0], (str, bytes)):
        path = Path(args[0].decode() if isinstance(args[0], bytes) else args[0])
        if path.name == ".env" or path.name.startswith(".env.") or path.name in {"credentials", "id_rsa", "id_ed25519"} or any(part in {".aws", ".ssh"} for part in path.parts):
            raise RuntimeError("Credential access is forbidden")


sys.addaudithook(guard)
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from arbos_voice import call_state as call  # noqa: E402
from arbos_voice import vision_state as vision  # noqa: E402


def data(value):
    if is_dataclass(value):
        return {"_tag": type(value).__name__, **{field.name: data(getattr(value, field.name)) for field in fields(value)}}
    if isinstance(value, tuple):
        return [data(item) for item in value]
    return value


def index(value):
    if type(value) is not int or not 0 <= value < MAX_EVENTS:
        raise ValueError("Invalid token reference")
    return value


def token(tokens, event):
    position = index(event["token"])
    return tokens[position % len(tokens)] if tokens else 0


def mode(value):
    if value == "group":
        return call.Group()
    if value in {"alice", "bob"}:
        return call.Duplex("u_" + value)
    raise ValueError("Invalid synthetic mode")


def trace(request):
    machine, events = request["machine"], request["events"]
    if machine not in {"vision", "call", "mute", "mode"}:
        raise ValueError("Invalid machine")
    if not isinstance(events, list) or len(events) > MAX_EVENTS:
        raise ValueError("Invalid trace size")
    state = vision.Enabled() if machine == "vision" else call.Connecting() if machine == "call" else call.Unapplied(False if machine == "mute" else None)
    initial, steps, tokens = data(state), [], []
    for event in events:
        if not isinstance(event, dict):
            raise ValueError("Invalid event")
        kind, result, previous = event["kind"], None, state
        if machine == "vision":
            if kind in {"enable", "disable", "close"} and set(event) == {"kind"}:
                state = vision.transition(state, kind)
            elif kind == "capture" and set(event) == {"kind"}:
                result = vision.capture_generation(state)
                tokens.append(result)
            elif kind == "frame" and set(event) == {"kind", "token"}:
                result = vision.accepts_frame(state, token(tokens, event))
            else:
                raise ValueError("Invalid vision event")
        elif machine == "call":
            if kind == "progress" and set(event) == {"kind", "stage"} and event["stage"] in {"waiting", "starting", "running"}:
                state = call.lifecycle_transition(state, call.StartupProgress(event["stage"]))
            elif kind in {"stop", "prestop"} and set(event) == {"kind", "reason"} and event["reason"] in {"asked", "shutdown", "room_closed"}:
                constructor = call.StopRequested if kind == "stop" else call.PreSessionStopped
                state = call.lifecycle_transition(state, constructor(event["reason"]))
            elif kind == "finalize" and set(event) <= {"kind", "token", "failed"} and {"kind", "token"} <= set(event):
                failed = event.get("failed", [])
                if not isinstance(failed, list) or len(failed) > len(call.CLEANUP_STEPS) or any(item not in call.CLEANUP_STEPS for item in failed):
                    raise ValueError("Invalid cleanup steps")
                state = call.lifecycle_transition(state, call.Finalized(token(tokens, event), tuple(failed)))
            else:
                raise ValueError("Invalid call event")
            if kind == "stop" and state is not previous and isinstance(state, call.Finishing):
                result = state.attempt
                tokens.append(result)
        elif machine in {"mute", "mode"}:
            if kind == "request" and set(event) == {"kind", "value"}:
                value = event["value"]
                if machine == "mute" and type(value) is not bool:
                    raise ValueError("Invalid mute target")
                state = call.apply_transition(state, call.ApplyRequested(value if machine == "mute" else mode(value)))
                if state is not previous and isinstance(state, call.Applying):
                    result = state.attempt
                    tokens.append(result)
            elif kind in {"success", "error"} and set(event) == {"kind", "token"}:
                constructor = call.ApplySucceeded if kind == "success" else call.ApplyErrored
                state = call.apply_transition(state, constructor(token(tokens, event)))
            else:
                raise ValueError("Invalid apply event")
        else:
            raise ValueError("Invalid machine")
        step = {"state": data(state), "result": result}
        if machine in {"mute", "mode"}:
            step.update(confirmed=data(call.confirmed_value(state)), requested=data(call.requested_value(state)))
        steps.append(step)
    return {"id": request["id"], "initial": initial, "steps": steps}


def main():
    for line in iter(lambda: sys.stdin.buffer.readline(MAX_LINE + 1), b""):
        request_id = None
        try:
            if len(line) > MAX_LINE:
                while line and not line.endswith(b"\n"):
                    line = sys.stdin.buffer.readline(MAX_LINE + 1)
                raise ValueError("Oversized line")
            request = json.loads(line)
            if not isinstance(request, dict) or set(request) != {"id", "machine", "events"}:
                raise ValueError("Invalid request")
            request_id = request["id"]
            if type(request_id) is not int or not 0 <= request_id <= 9_007_199_254_740_991:
                request_id = None
                raise ValueError("Invalid id")
            response = trace(request)
        except (ValueError, TypeError, KeyError, RecursionError):
            response = {"id": request_id, "error": "Invalid trace"}
        print(json.dumps(response, ensure_ascii=True, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
