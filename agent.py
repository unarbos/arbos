import json
import os
import subprocess
import sys
import time
from pathlib import Path
from datetime import datetime

PROMPT_FILE = Path(__file__).parent / "PROMPT.md"
WORKING_DIR = Path(__file__).parent
HISTORY_DIR = WORKING_DIR / "history"

# ── Colors ───────────────────────────────────────────────────────────────────

if sys.stdout.isatty():
    GREEN = '\033[0;32m'
    RED = '\033[0;31m'
    CYAN = '\033[0;36m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    NC = '\033[0m'
else:
    GREEN = RED = CYAN = BOLD = DIM = NC = ''

_log_fh = None


def _file_log(msg: str):
    if _log_fh:
        ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        _log_fh.write(f"{ts}  {msg}\n")
        _log_fh.flush()


def ok(msg: str):
    print(f"  {GREEN}+{NC} {msg}", flush=True)
    _file_log(f"+  {msg}")


def err(msg: str):
    print(f"  {RED}x{NC} {msg}", flush=True)
    _file_log(f"x  {msg}")


def header(msg: str):
    print(f"\n  {BOLD}{msg}{NC}\n", flush=True)
    _file_log(f"── {msg}")


def dim(msg: str):
    print(f"  {DIM}{msg}{NC}", flush=True)
    _file_log(f"   {msg}")


def info(msg: str):
    print(f"  {CYAN}·{NC} {msg}", flush=True)
    _file_log(f"·  {msg}")


def banner():
    print(f"\n{CYAN}{BOLD}", end="")
    print("      _                    _   ")
    print("     / \\   __ _  ___ _ __ | |_ ")
    print("    / _ \\ / _` |/ _ \\ '_ \\| __|")
    print("   / ___ \\ (_| |  __/ | | | |_ ")
    print("  /_/   \\_\\__, |\\___|_| |_|\\__|")
    print("          |___/                ")
    print(f"{NC}")


def fmt_duration(seconds: float) -> str:
    if seconds < 60:
        return f"{seconds:.1f}s"
    m, s = divmod(int(seconds), 60)
    return f"{m}m {s}s"


def load_prompt() -> str:
    if not PROMPT_FILE.exists():
        err(f"Prompt file not found: {PROMPT_FILE}")
        sys.exit(1)
    text = PROMPT_FILE.read_text().strip()
    if not text:
        err(f"Prompt file is empty: {PROMPT_FILE}")
        sys.exit(1)
    return text


def make_run_dir() -> Path:
    HISTORY_DIR.mkdir(exist_ok=True)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_dir = HISTORY_DIR / ts
    run_dir.mkdir(parents=True, exist_ok=True)
    return run_dir


def _describe_tool_call(tc: dict) -> str:
    for key, val in tc.items():
        if not isinstance(val, dict):
            continue
        args = val.get("args", {})
        if "path" in args:
            return f"{key}({args['path']})"
        if "command" in args:
            cmd = args["command"]
            return f"{key}({cmd[:80]}{'…' if len(cmd) > 80 else ''})"
        if "pattern" in args:
            return f"{key}(pattern={args['pattern']!r})"
        arg_summary = ", ".join(f"{k}={v!r}" for k, v in list(args.items())[:2])
        return f"{key}({arg_summary})"
    return str(list(tc.keys()))


def run_agent(cmd: list[str], phase: str, output_file: Path) -> subprocess.CompletedProcess:
    stream_cmd = []
    for arg in cmd:
        if arg == "--output-format":
            stream_cmd.append(arg)
            continue
        if stream_cmd and stream_cmd[-1] == "--output-format":
            stream_cmd.append("stream-json")
            continue
        stream_cmd.append(arg)
    if "--stream-partial-output" not in stream_cmd:
        stream_cmd.insert(-1, "--stream-partial-output")

    api_key = os.environ.get("CURSOR_API_KEY")
    if api_key and "--api-key" not in stream_cmd:
        stream_cmd.insert(1, "--api-key")
        stream_cmd.insert(2, api_key)

    dim(f"running: {' '.join(stream_cmd[:6])}{'…' if len(stream_cmd) > 6 else ''}")
    t0 = time.monotonic()

    proc = subprocess.Popen(
        stream_cmd, cwd=WORKING_DIR,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, bufsize=1,
    )

    result_text = ""
    raw_lines: list[str] = []
    for line in iter(proc.stdout.readline, ""):
        raw_lines.append(line)
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            continue

        etype = evt.get("type")
        subtype = evt.get("subtype")

        if etype == "tool_call" and subtype == "started":
            desc = _describe_tool_call(evt.get("tool_call", {}))
            info(f"{phase} tool call  {desc}")
        elif etype == "tool_call" and subtype == "completed":
            desc = _describe_tool_call(evt.get("tool_call", {}))
            ok(f"{phase} tool done  {desc}")
        elif etype == "assistant":
            text = ""
            for block in evt.get("message", {}).get("content", []):
                if isinstance(block, dict) and block.get("type") == "text":
                    text += block.get("text", "")
            if text.strip():
                for tline in text.strip().splitlines():
                    dim(f"[{phase}] {tline}")
        elif etype == "result":
            result_text = evt.get("result", "")
            dur = evt.get("duration_ms", 0)
            usage = evt.get("usage", {})
            ok(
                f"{phase} done  {fmt_duration(dur / 1000)}"
                f"  in={usage.get('inputTokens', '?')}"
                f"  out={usage.get('outputTokens', '?')}"
            )

    stderr_output = proc.stderr.read() if proc.stderr else ""
    returncode = proc.wait()
    elapsed = time.monotonic() - t0
    output_file.write_text("".join(raw_lines))

    if returncode == 0:
        ok(f"{phase} finished  rc={returncode}  {fmt_duration(elapsed)}")
    else:
        err(f"{phase} finished  rc={returncode}  {fmt_duration(elapsed)}")
        if stderr_output.strip():
            for sline in stderr_output.strip().splitlines()[:20]:
                err(f"  stderr: {sline}")

    return subprocess.CompletedProcess(
        args=cmd, returncode=returncode,
        stdout=result_text, stderr=stderr_output,
    )


def extract_text(result: subprocess.CompletedProcess) -> str:
    output = result.stdout or ""
    if not output.strip():
        output = result.stderr or "(no output)"
    return output


def run_step(prompt: str) -> bool:
    global _log_fh

    run_dir = make_run_dir()
    t0 = time.monotonic()

    log_file = run_dir / "logs.txt"
    _log_fh = open(log_file, "a", encoding="utf-8")

    dim(f"run dir  {run_dir}")
    dim(f"log file {log_file}")

    # ── Plan ──
    header("Planning")

    preview = prompt[:200] + ("…" if len(prompt) > 200 else "")
    dim(f"prompt preview: {preview}")

    plan_result = run_agent(
        ["agent", "-p", "--force", "--mode", "plan", "--output-format", "text", prompt],
        phase="plan",
        output_file=run_dir / "plan_output.txt",
    )

    plan_text = extract_text(plan_result)
    (run_dir / "plan.md").write_text(plan_text)
    ok(f"Plan saved → {run_dir / 'plan.md'} ({len(plan_text)} chars)")

    if plan_result.returncode != 0:
        err(f"Plan phase exited with code {plan_result.returncode} — skipping execution")
        _log_fh.close()
        _log_fh = None
        return False

    # ── Execute ──
    header("Execution")

    execute_prompt = (
        f"Here is the plan that was previously generated:\n\n"
        f"---\n{plan_text}\n---\n\n"
        f"Now implement this plan. The original request was:\n\n{prompt}"
    )
    dim(f"prompt size: {len(execute_prompt)} chars (plan={len(plan_text)} + original={len(prompt)})")

    exec_result = run_agent(
        ["agent", "-p", "--force", "--output-format", "text", execute_prompt],
        phase="exec",
        output_file=run_dir / "exec_output.txt",
    )

    exec_text = extract_text(exec_result)
    (run_dir / "rollout.md").write_text(exec_text)
    ok(f"Rollout saved → {run_dir / 'rollout.md'} ({len(exec_text)} chars)")

    elapsed = time.monotonic() - t0
    success = exec_result.returncode == 0
    if not success:
        err(f"Execution phase exited with code {exec_result.returncode}")
    else:
        ok("Run completed successfully")

    dim(f"total duration: {fmt_duration(elapsed)}")

    _log_fh.close()
    _log_fh = None
    return success


def main() -> None:
    banner()
    header("Agent loop")

    dim(f"prompt   {PROMPT_FILE}")
    dim(f"workdir  {WORKING_DIR}")
    dim(f"history  {HISTORY_DIR}")

    loop_count = 0
    consecutive_failures = 0
    while True:
        loop_count += 1
        prompt = load_prompt()
        header(f"Iteration {loop_count}")
        dim(f"prompt={len(prompt)} chars")
        success = run_step(prompt)
        if success:
            consecutive_failures = 0
        else:
            consecutive_failures += 1
            delay = min(2 ** consecutive_failures, 120)
            err(f"Backing off for {delay}s after {consecutive_failures} consecutive failure(s)")
            time.sleep(delay)


if __name__ == "__main__":
    main()
