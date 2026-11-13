"""Bounded observations of the actual pure feedback transitions, without GH effects."""
from dataclasses import fields
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
import feedback_state as machine  # noqa: E402

COMMIT_STATES = {kind.__name__: kind for kind in (
    machine.Candidate, machine.Held, machine.AwaitingApproval, machine.GatePending,
    machine.MissingHead, machine.StaleHead, machine.ReadyToMerge, machine.Landed, machine.InvalidControl,
)}
DELIVERY_STATES = {kind.__name__: kind for kind in (
    machine.DeliveryPending, machine.AwaitingArtifacts, machine.AwaitingDeploy, machine.Shipped, machine.InvalidControl,
)}
COMMIT_EVENTS = {kind.__name__: kind for kind in (machine.ObserveGate, machine.Merged)}
DELIVERY_EVENTS = {kind.__name__: kind for kind in (machine.ObserveDelivery, machine.Deployed)}


def snapshot(state):
    return {"_tag": type(state).__name__, **{field.name: getattr(state, field.name) for field in fields(state)}}


def value(item, variants):
    if not isinstance(item, dict) or not isinstance(item.get("_tag"), str) or item["_tag"] not in variants:
        raise ValueError("Invalid variant")
    kind = variants[item["_tag"]]
    names = {field.name for field in fields(kind)}
    if set(item) != {"_tag", *names}:
        raise ValueError("Invalid fields")
    payload = {name: item[name] for name in names}
    for name, entry in payload.items():
        if name == "pending_deploy":
            if not isinstance(entry, list) or len(entry) > 3 or any(not isinstance(v, str) or len(v) > 32 for v in entry):
                raise ValueError("Invalid deployment list")
            payload[name] = tuple(entry)
        elif name in {"green", "eligible", "held", "approval_required", "waiting_ios"}:
            if type(entry) is not bool:
                raise ValueError("Invalid flag")
        elif entry is None and name in {"covered_head", "live_head"}:
            pass
        elif not isinstance(entry, str) or len(entry) > 512:
            raise ValueError("Invalid text field")
    return kind(**payload)


def trace(request):
    if not isinstance(request, dict) or set(request) != {"id", "machine", "state", "events"}:
        raise ValueError("Invalid trace fields")
    if type(request["id"]) is not int or not 0 <= request["id"] <= 2**53 - 1:
        raise ValueError("Invalid id")
    domain = request["machine"]
    if domain not in ("commit", "delivery"):
        raise ValueError("Invalid machine")
    events = request["events"]
    if not isinstance(events, list) or len(events) > 64:
        raise ValueError("Invalid trace size")
    states, inputs, transition = ((COMMIT_STATES, COMMIT_EVENTS, machine.commit_transition) if domain == "commit"
                                 else (DELIVERY_STATES, DELIVERY_EVENTS, machine.delivery_transition))
    state = value(request["state"], states)
    start, steps = snapshot(state), []
    for item in events:
        state = transition(state, value(item, inputs))
        steps.append(snapshot(state))
    return {"id": request["id"], "initial": start, "steps": steps}


def unique_object(pairs):
    output = {}
    for key, entry in pairs:
        if key in output:
            raise ValueError("Duplicate field")
        output[key] = entry
    return output


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
            request = json.loads(line, object_pairs_hook=unique_object)
            response = trace(request)
        except (ValueError, TypeError, KeyError, RecursionError):
            identity = request.get("id") if isinstance(request, dict) else None
            response = {"id": identity if type(identity) is int and 0 <= identity <= 2**53 - 1 else None,
                        "error": "Invalid trace"}
        print(json.dumps(response, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
