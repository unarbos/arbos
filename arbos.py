import json
import os
import subprocess
import sys
import time
import threading
from pathlib import Path
from datetime import datetime

from dotenv import load_dotenv
import requests

WORKING_DIR = Path(__file__).parent
load_dotenv(WORKING_DIR / ".env")

PROMPT_FILE = WORKING_DIR / "PROMPT.md"
CONTEXT_DIR = WORKING_DIR / "context"
GOAL_FILE = CONTEXT_DIR / "GOAL.md"
STATE_FILE = CONTEXT_DIR / "STATE.md"
INBOX_FILE = CONTEXT_DIR / "INBOX.md"
RUNS_DIR = CONTEXT_DIR / "runs"
CHATLOG_DIR = CONTEXT_DIR / "chat"
RESTART_FLAG = WORKING_DIR / ".restart"
CHAT_ID_FILE = WORKING_DIR / "chat_id.txt"
STEP_UPDATE_CHAR_LIMIT = 500
STEP_SOURCE_CHAR_LIMIT = 3500
STEP_SUMMARY_MODEL = "gpt-5.3-codex-low-fast"

_log_fh = None
_log_lock = threading.Lock()
_agent_wake = threading.Event()


def _file_log(msg: str):
    with _log_lock:
        if _log_fh:
            ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
            _log_fh.write(f"{ts}  {msg}\n")
            _log_fh.flush()


def _log(msg: str, *, blank: bool = False):
    if blank:
        print(flush=True)
    print(msg, flush=True)
    _file_log(msg)


def fmt_duration(seconds: float) -> str:
    if seconds < 60:
        return f"{seconds:.1f}s"
    m, s = divmod(int(seconds), 60)
    return f"{m}m {s}s"


# ── Prompt helpers ───────────────────────────────────────────────────────────

def load_prompt(consume_inbox: bool = False) -> str:
    """Build full prompt: PROMPT.md + GOAL.md + STATE.md + INBOX.md + chatlog."""
    parts = []
    if PROMPT_FILE.exists():
        text = PROMPT_FILE.read_text().strip()
        if text:
            parts.append(text)
    if GOAL_FILE.exists():
        goal_text = GOAL_FILE.read_text().strip()
        if goal_text:
            parts.append(f"## Goal\n\n{goal_text}")
    if STATE_FILE.exists():
        state_text = STATE_FILE.read_text().strip()
        if state_text:
            parts.append(f"## State\n\n{state_text}")
    if INBOX_FILE.exists():
        inbox_text = INBOX_FILE.read_text().strip()
        if inbox_text:
            parts.append(f"## Inbox\n\n{inbox_text}")
        if consume_inbox:
            INBOX_FILE.write_text("")
    chatlog = load_chatlog()
    if chatlog:
        parts.append(chatlog)
    return "\n\n".join(parts)


def make_run_dir() -> Path:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    run_dir = RUNS_DIR / ts
    run_dir.mkdir(parents=True, exist_ok=True)
    return run_dir


def log_chat(role: str, text: str):
    """Append to chatlog, rolling to a new file when size exceeds limit."""
    CHATLOG_DIR.mkdir(parents=True, exist_ok=True)
    max_file_size = 4000
    max_files = 50

    existing = sorted(CHATLOG_DIR.glob("*.jsonl"))

    current: Path | None = None
    if existing and existing[-1].stat().st_size < max_file_size:
        current = existing[-1]

    if current is None:
        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        current = CHATLOG_DIR / f"{ts}.jsonl"

    entry = json.dumps({"role": role, "text": text[:1000], "ts": datetime.now().isoformat()})
    with open(current, "a", encoding="utf-8") as f:
        f.write(entry + "\n")

    all_files = sorted(CHATLOG_DIR.glob("*.jsonl"))
    for old in all_files[:-max_files]:
        old.unlink(missing_ok=True)


def load_chatlog(max_chars: int = 8000) -> str:
    """Load recent Telegram chat history."""
    if not CHATLOG_DIR.exists():
        return ""
    files = sorted(CHATLOG_DIR.glob("*.jsonl"))
    if not files:
        return ""

    lines: list[str] = []
    total = 0
    for f in reversed(files):
        for raw in reversed(f.read_text().strip().splitlines()):
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            entry = f"[{msg.get('ts', '?')[:16]}] {msg['role']}: {msg['text']}"
            if total + len(entry) > max_chars:
                lines.reverse()
                return "## Recent Telegram chat\n\n" + "\n".join(lines)
            lines.append(entry)
            total += len(entry) + 1

    lines.reverse()
    if not lines:
        return ""
    return "## Recent Telegram chat\n\n" + "\n".join(lines)


# ── Step update helpers ──────────────────────────────────────────────────────

def _clip_text(text: str, max_chars: int) -> str:
    text = text.strip()
    if len(text) <= max_chars:
        return text
    keep = max((max_chars - 5) // 2, 1)
    return f"{text[:keep]}\n...\n{text[-keep:]}"


def _normalize_step_update(text: str, *, step_number: int, success: bool) -> str:
    cleaned = " ".join((text or "").split())
    if not cleaned:
        status = "success" if success else "failed"
        cleaned = f"Step {step_number}: {status}; no summary generated."
    if len(cleaned) > STEP_UPDATE_CHAR_LIMIT:
        cleaned = cleaned[: STEP_UPDATE_CHAR_LIMIT - 1].rstrip() + "…"
    return cleaned


def _fallback_step_update(
    *,
    step_number: int,
    success: bool,
    plan_text: str,
    rollout_text: str,
    logs_text: str,
) -> str:
    status = "success" if success else "failed"

    def first_line(text: str) -> str:
        for line in text.splitlines():
            cleaned = line.strip().lstrip("-*0123456789. ")
            if cleaned:
                return cleaned
        return ""

    action = first_line(rollout_text) or first_line(plan_text) or first_line(logs_text) or "completed a step"
    return _normalize_step_update(
        f"Step {step_number}: {status}; {action}.",
        step_number=step_number,
        success=success,
    )


def _generate_step_update(*, step_number: int, success: bool, run_dir: Path) -> str:
    plan_text = (run_dir / "plan.md").read_text() if (run_dir / "plan.md").exists() else ""
    rollout_text = (run_dir / "rollout.md").read_text() if (run_dir / "rollout.md").exists() else ""
    logs_text = (run_dir / "logs.txt").read_text() if (run_dir / "logs.txt").exists() else ""

    prompt = (
        "Summarize one completed agent step as a Telegram update.\n"
        f"Return plain text only, max {STEP_UPDATE_CHAR_LIMIT} characters total.\n"
        "Include:\n"
        "- step number\n"
        "- whether it succeeded or failed\n"
        "- the main action taken\n"
        "- blocker or next action if visible\n"
        "No markdown, no code fences.\n\n"
        f"Step number: {step_number}\n"
        f"Outcome: {'success' if success else 'failure'}\n"
        f"Plan:\n{_clip_text(plan_text, STEP_SOURCE_CHAR_LIMIT) or '(empty)'}\n\n"
        f"Rollout:\n{_clip_text(rollout_text, STEP_SOURCE_CHAR_LIMIT)}\n\n"
        f"Logs:\n{_clip_text(logs_text, STEP_SOURCE_CHAR_LIMIT)}"
    )

    result = run_agent(
        ["agent", "-p", "--force", "--mode", "plan", "--model", STEP_SUMMARY_MODEL, "--output-format", "text", prompt],
        phase="summary",
        output_file=run_dir / "summary_output.txt",
    )
    summary_text = extract_text(result)
    if result.returncode != 0:
        return _fallback_step_update(
            step_number=step_number,
            success=success,
            plan_text=plan_text,
            rollout_text=rollout_text,
            logs_text=logs_text,
        )

    return _normalize_step_update(summary_text, step_number=step_number, success=success)


def _step_update_target() -> tuple[str, str] | None:
    token = os.getenv("TAU_BOT_TOKEN")
    if not token:
        _log("step update skipped: TAU_BOT_TOKEN not set")
        return None
    if not CHAT_ID_FILE.exists():
        _log("step update skipped: chat_id.txt not found")
        return None
    chat_id = CHAT_ID_FILE.read_text().strip()
    if not chat_id:
        _log("step update skipped: empty chat_id.txt")
        return None
    return token, chat_id


def _send_telegram_text(text: str, *, target: tuple[str, str] | None = None) -> bool:
    target = target or _step_update_target()
    if not target:
        return False
    token, chat_id = target
    try:
        response = requests.post(
            f"https://api.telegram.org/bot{token}/sendMessage",
            json={"chat_id": chat_id, "text": text[:4000]},
            timeout=15,
        )
        response.raise_for_status()
    except Exception as exc:
        _log(f"step update send failed: {str(exc)[:120]}")
        return False
    log_chat("bot", text[:1000])
    _log("step update sent to Telegram")
    return True


def _send_step_update(step_number: int, run_dir: Path, success: bool):
    target = _step_update_target()
    if not target:
        return
    summary_text = _generate_step_update(
        step_number=step_number, success=success, run_dir=run_dir,
    )
    _send_telegram_text(summary_text, target=target)


# ── Agent runner ─────────────────────────────────────────────────────────────

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

    preview = " ".join(stream_cmd[:6]) + ("…" if len(stream_cmd) > 6 else "")
    _log(f"{phase}: starting {preview}")
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
        if evt.get("type") == "result":
            result_text = evt.get("result", "")

    stderr_output = proc.stderr.read() if proc.stderr else ""
    returncode = proc.wait()
    elapsed = time.monotonic() - t0
    output_file.write_text("".join(raw_lines))

    _log(f"{phase}: finished rc={returncode} {fmt_duration(elapsed)}")
    if returncode != 0 and stderr_output.strip():
        _log(f"{phase}: stderr {stderr_output.strip()[:300]}")

    return subprocess.CompletedProcess(
        args=cmd, returncode=returncode,
        stdout=result_text, stderr=stderr_output,
    )


def extract_text(result: subprocess.CompletedProcess) -> str:
    output = result.stdout or ""
    if not output.strip():
        output = result.stderr or "(no output)"
    return output


def run_step(prompt: str, step_number: int) -> bool:
    global _log_fh

    run_dir = make_run_dir()
    t0 = time.monotonic()

    log_file = run_dir / "logs.txt"
    with _log_lock:
        _log_fh = open(log_file, "a", encoding="utf-8")

    success = False
    try:
        _log(f"run dir {run_dir}")

        preview = prompt[:200] + ("…" if len(prompt) > 200 else "")
        _log(f"prompt preview: {preview}")

        plan_result = run_agent(
            ["agent", "-p", "--force", "--mode", "plan", "--output-format", "text", prompt],
            phase="plan",
            output_file=run_dir / "plan_output.txt",
        )

        plan_text = extract_text(plan_result)
        (run_dir / "plan.md").write_text(plan_text)
        _log(f"plan saved ({len(plan_text)} chars)")

        if plan_result.returncode != 0:
            _log(f"plan phase exited with code {plan_result.returncode}; skipping execution")
            return False

        execute_prompt = (
            f"Here is the plan that was previously generated:\n\n"
            f"---\n{plan_text}\n---\n\n"
            f"Now implement this plan. The original request was:\n\n{prompt}"
        )

        exec_result = run_agent(
            ["agent", "-p", "--force", "--output-format", "text", execute_prompt],
            phase="exec",
            output_file=run_dir / "exec_output.txt",
        )

        exec_text = extract_text(exec_result)
        (run_dir / "rollout.md").write_text(exec_text)
        _log(f"rollout saved ({len(exec_text)} chars)")

        elapsed = time.monotonic() - t0
        success = exec_result.returncode == 0
        _log(f"step {'succeeded' if success else 'failed'} in {fmt_duration(elapsed)}")
        return success
    finally:
        with _log_lock:
            if _log_fh:
                _log_fh.close()
                _log_fh = None
        try:
            _send_step_update(step_number, run_dir, success)
        except Exception as exc:
            _log(f"step update failed: {str(exc)[:120]}")


# ── Agent loop ───────────────────────────────────────────────────────────────

def agent_loop():
    step_count = 0
    failures = 0

    while True:
        if not GOAL_FILE.exists() or not GOAL_FILE.read_text().strip():
            _agent_wake.wait(timeout=5)
            _agent_wake.clear()
            continue

        step_count += 1
        _log(f"Step {step_count}", blank=True)

        prompt = load_prompt(consume_inbox=True)
        if not prompt:
            continue

        _log(f"prompt={len(prompt)} chars")

        success = run_step(prompt, step_count)

        if success:
            failures = 0
        else:
            failures += 1
            _log(f"failure #{failures}")

        delay = int(os.environ.get("AGENT_DELAY", "60"))
        effective_delay = delay + min(2 ** failures, 120) * (1 if failures else 0)
        _agent_wake.wait(timeout=effective_delay)
        _agent_wake.clear()


# ── Telegram bot ─────────────────────────────────────────────────────────────

def _recent_context(max_chars: int = 6000) -> str:
    if not RUNS_DIR.exists():
        return ""
    run_dirs = sorted(
        [d for d in RUNS_DIR.iterdir() if d.is_dir()],
        key=lambda d: d.name, reverse=True,
    )
    parts: list[str] = []
    total = 0
    for run_dir in run_dirs:
        for name in ("plan.md", "rollout.md"):
            f = run_dir / name
            if f.exists():
                content = f.read_text()[:2000]
                hdr = f"\n--- {name} ({run_dir.name}) ---\n"
                if total + len(hdr) + len(content) > max_chars:
                    return "".join(parts)
                parts.append(hdr + content)
                total += len(hdr) + len(content)
        if total > max_chars:
            break
    return "".join(parts)


def _build_operator_prompt(user_text: str) -> str:
    """Build prompt for the CLI agent to handle any operator request."""
    goal = GOAL_FILE.read_text().strip() if GOAL_FILE.exists() else "(no goal set)"
    state = STATE_FILE.read_text().strip()[:500] if STATE_FILE.exists() else "(no state)"

    context = _recent_context(max_chars=4000)
    chatlog = load_chatlog(max_chars=4000)

    parts = [
        "You are the operator interface for Arbos, a coding agent running in a loop via pm2.\n"
        "The operator communicates with you through Telegram. Be concise and direct.\n"
        "When the operator asks you to do something, do it by modifying the relevant files.\n"
        "When the operator asks a question, answer from the available context.\n\n"
        "## Available operations\n\n"
        "- **Set goal**: write to `context/GOAL.md`. The agent loop runs while this file is non-empty.\n"
        "- **Clear goal / stop**: empty `context/GOAL.md` to pause the agent loop.\n"
        "- **Update state**: write to `context/STATE.md`.\n"
        "- **Message the agent**: append a timestamped line to `context/INBOX.md`.\n"
        "- **Set system prompt**: write to `PROMPT.md`.\n"
        "- **Set env variable**: update `.env` (e.g. `AGENT_DELAY=120` to change step interval).\n"
        "- **View logs**: read files in `context/runs/<timestamp>/` (plan.md, rollout.md, logs.txt).\n"
        "- **Modify code & restart**: edit code files, then run `touch .restart`.\n"
        "- **Send follow-up**: run `python tools/send_telegram.py \"message\"`.",
        f"## Current goal\n{goal}",
        f"## Current state\n{state}",
    ]
    if chatlog:
        parts.append(chatlog)
    if context:
        parts.append(f"## Recent activity\n{context}")
    parts.append(f"## Operator message\n{user_text}")

    return "\n\n".join(parts)


def run_agent_streaming(bot, prompt: str, chat_id: int, *, execute: bool = False) -> str:
    """Run the Cursor agent CLI and stream output into a Telegram message."""
    cmd = [
        "agent", "-p", "--force",
        "--output-format", "stream-json",
        "--stream-partial-output",
    ]
    if not execute:
        cmd.extend(["--mode", "plan"])

    api_key = os.environ.get("CURSOR_API_KEY")
    if api_key:
        cmd.insert(1, "--api-key")
        cmd.insert(2, api_key)

    cmd.append(prompt)

    msg = bot.send_message(chat_id, "Running...")
    current_text = ""
    last_edit = 0.0

    def _edit(text: str, force: bool = False):
        nonlocal last_edit
        now = time.time()
        if not force and now - last_edit < 1.5:
            return
        display = text[-3800:] if len(text) > 3800 else text
        if not display.strip():
            return
        try:
            bot.edit_message_text(display, chat_id, msg.message_id)
            last_edit = now
        except Exception:
            pass

    try:
        proc = subprocess.Popen(
            cmd, cwd=WORKING_DIR,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )

        for line in iter(proc.stdout.readline, ""):
            try:
                evt = json.loads(line)
            except json.JSONDecodeError:
                continue

            etype = evt.get("type")

            if etype == "assistant":
                for block in evt.get("message", {}).get("content", []):
                    if isinstance(block, dict) and block.get("type") == "text":
                        t = block.get("text", "")
                        if t:
                            current_text += t

            elif etype == "result":
                result_text = evt.get("result", "")
                if result_text.strip():
                    current_text = result_text

            _edit(current_text)

        proc.wait()
        _edit(current_text, force=True)

        if not current_text.strip():
            try:
                bot.edit_message_text("(no output)", chat_id, msg.message_id)
            except Exception:
                pass

    except Exception as e:
        try:
            bot.edit_message_text(f"Error: {str(e)[:300]}", chat_id, msg.message_id)
        except Exception:
            pass

    return current_text


def run_bot():
    """Run the Telegram bot."""
    token = os.getenv("TAU_BOT_TOKEN")
    if not token:
        _log("TAU_BOT_TOKEN not set; add it to .env and restart")
        sys.exit(1)

    import telebot
    bot = telebot.TeleBot(token)

    def _save_chat_id(chat_id: int):
        CHAT_ID_FILE.write_text(str(chat_id))

    @bot.message_handler(commands=["start"])
    def handle_start(message):
        _save_chat_id(message.chat.id)
        bot.send_message(
            message.chat.id,
            "Give me a goal and I'll work on it. You can also send me messages to update the goal, state, or inbox.",
        )

    @bot.message_handler(func=lambda m: True)
    def handle_message(message):
        _save_chat_id(message.chat.id)
        log_chat("user", message.text)
        prompt = _build_operator_prompt(message.text)

        def _run():
            response = run_agent_streaming(bot, prompt, message.chat.id, execute=True)
            log_chat("bot", response[:1000])
            _agent_wake.set()
            load_dotenv(WORKING_DIR / ".env", override=True)

        threading.Thread(target=_run, daemon=True).start()

    _log("telegram bot started")
    while True:
        try:
            bot.infinity_polling()
        except Exception as e:
            _log(f"bot polling error: {str(e)[:80]}, reconnecting in 5s")
            time.sleep(5)


# ── Main ─────────────────────────────────────────────────────────────────────

def main() -> None:
    _log(f"arbos starting in {WORKING_DIR}")
    CONTEXT_DIR.mkdir(parents=True, exist_ok=True)

    threading.Thread(target=agent_loop, daemon=True).start()
    threading.Thread(target=run_bot, daemon=True).start()

    while True:
        if RESTART_FLAG.exists():
            RESTART_FLAG.unlink()
            _log("restart requested; exiting for pm2")
            sys.exit(0)
        time.sleep(1)


if __name__ == "__main__":
    main()
