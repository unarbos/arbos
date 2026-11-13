"""Bounded observations of the actual pure publication transition, without GH effects."""
from dataclasses import fields
from enum import Enum
import json
from pathlib import Path
import sys


def guard(event, args):
    if event.startswith("socket.") or event in {"subprocess.Popen", "os.system"}:
        raise RuntimeError("External access is forbidden")
    if event == "open" and args and isinstance(args[0], (str, bytes)):
        path = Path(args[0].decode() if isinstance(args[0], bytes) else args[0])
        if any(part.lower().startswith(".env") or part in {".aws", ".ssh"} for part in path.parts):
            raise RuntimeError("Credential access is forbidden")


sys.addaudithook(guard)
sys.path.insert(0, str(Path(__file__).resolve().parent))
import publication as machine  # noqa: E402

STATES = {kind.__name__: kind for kind in (
    machine.Missing, machine.Draft, machine.Prerelease, machine.Stable,
    machine.Creating, machine.Uploading, machine.Finalizing, machine.Complete,
    machine.Refused, machine.Failed,
)}


def snapshot(state):
    return {"_tag": type(state).__name__, **{
        field.name: value.value if isinstance(value, Enum) else value
        for field in fields(state) for value in (getattr(state, field.name),)
    }}


def initial(value):
    if not isinstance(value, dict) or value.get("_tag") not in STATES:
        raise ValueError("Invalid state")
    kind = STATES[value["_tag"]]
    names = {field.name for field in fields(kind)}
    if set(value) != {"_tag", *names}:
        raise ValueError("Invalid state fields")
    if names == {"target"}:
        return kind(machine.Target(value["target"]))
    if names == {"error"}:
        return kind(machine.Error(value["error"]))
    return kind()


def event(value):
    if not isinstance(value, dict):
        raise ValueError("Invalid event")
    if set(value) == {"kind", "intent", "channel"} and value["kind"] == "request":
        if value["channel"] not in {"stable", "edge"}:
            raise ValueError("Invalid channel")
        return machine.Request(machine.Intent(value["intent"]), value["channel"])
    if set(value) == {"kind", "error"} and value["kind"] == "failure":
        return machine.Failure(machine.Error(value["error"]))
    if set(value) == {"kind"}:
        return machine.Event(value["kind"])
    raise ValueError("Invalid event fields")


def trace(request):
    if not isinstance(request, dict) or set(request) != {"id", "state", "events"}:
        raise ValueError("Invalid trace fields")
    if type(request["id"]) is not int or not 0 <= request["id"] <= 2**53 - 1:
        raise ValueError("Invalid id")
    events = request["events"]
    if not isinstance(events, list) or len(events) > 64:
        raise ValueError("Invalid trace size")
    state = initial(request["state"])
    start, steps = snapshot(state), []
    for value in events:
        state = machine.transition(state, event(value))
        steps.append(snapshot(state))
    return {"id": request["id"], "initial": start, "steps": steps}


def main():
    while True:
        line = sys.stdin.buffer.readline(65_538)
        if not line:
            return
        request = None
        try:
            if len(line) > 65_536:
                while not line.endswith(b"\n"):
                    line = sys.stdin.buffer.readline(65_538)
                    if not line:
                        break
                raise ValueError("Trace too large")
            request = json.loads(line, object_pairs_hook=machine.unique_object)
            response = trace(request)
        except (ValueError, TypeError, KeyError, RecursionError):
            identity = request.get("id") if isinstance(request, dict) else None
            response = {"id": identity if type(identity) is int and 0 <= identity <= 2**53 - 1 else None,
                        "error": "Invalid trace"}
        print(json.dumps(response, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
