"""The narrator: speaks highlights of the main agent's work, not the screen.

One per call. It listens to the kernel attach stream the gateway already
holds, decides what is worth a sentence (a turn's result, a sub-agent
finishing, a question, an error), cuts it to a spoken length, and queues it
for the gateway's own TTS. Everything it says also goes to the client as a
`narrator.say` frame so the desktop can write it into the transcript.

"More detail" questions are answered from the record: the narrator reads
the relevant transcript through the kernel's `tail`/`read` frames and
answers in a few sentences. A narrator model (OpenRouter) polishes that
when one is configured; without it the extractive answer is spoken as is,
which keeps the test harness deterministic.
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
import time
from dataclasses import dataclass, field
from typing import Awaitable, Callable

import httpx

from . import protocol as P
from .kernel import KernelClient
from .tts import speakable

log = logging.getLogger("voice.narrator")

HIGHLIGHT_CAP = 240
REPORT_CAP = 200
ERROR_CAP = 160
DETAIL_CAP = 420
COALESCE_S = 3.0
QUEUE_CAP = 3
# A line waits this long for the caller (or the speech model) to finish before it is spoken. Long:
# the narrator never talks over the caller; a report can wait, and the queue is capped anyway.
YIELD_TO_USER_S = 30.0
# An utterance waits this long for the speech model to claim it (a `more_detail` call) before it
# goes to the main agent. The duplex model's tool calls land well inside this.
FORWARD_GRACE_S = 1.5
# Said when an utterance goes to the main agent, so the caller knows it landed. The main agent's
# own reply follows as a highlight several seconds later.
ACK = "On it."

_SENTENCE = re.compile(r"(?<=[.!?])\s+")
_RESULT_WORDS = re.compile(
    r"\b(fail|failed|pass|passed|green|red|done|fixed|error|because|found|opened|merged|PR|pull request|"
    r"result|broke|works|sent|spawned|dispatched|finished|started)\b",
    re.I,
)
_FENCE = re.compile(r"```.*?```", re.S)
_DIFF_LINE = re.compile(r"^[+-]{1,3}\s|^@@ ", re.M)
_LIST_LINE = re.compile(r"^\s*(?:[-*•]|\d+\.)\s+", re.M)
_URL = re.compile(r"https?://\S+")
_DONE_PREFIX = re.compile(r"^Turn (ended badly|ended)\.\s*Last words:\s*", re.S)
_TRANSCRIPT_NOTE = re.compile(r"\s*\(transcript:[^)]*\)\s*$", re.S)

SpeakHook = Callable[[str], Awaitable[None]]
EmitHook = Callable[..., None]


@dataclass
class Line:
    """One thing to say."""

    kind: str  # highlight | report | ask | error | detail | status
    text: str
    ref: str = ""
    at: float = field(default_factory=time.monotonic)
    heard: bool | None = None  # None until spoken; False when interrupted


def highlight(text: str, cap: int = HIGHLIGHT_CAP, screen: str = "on your screen") -> str:
    """The spoken form of a long reply: the first sentence, plus the first later one that names a
    result, cut to `cap`. When anything was left out (more sentences, code, a diff, a list, a link)
    the line ends with where the rest is."""
    raw = text.strip()
    if not raw:
        return ""
    had_extra = bool(_FENCE.search(raw) or _DIFF_LINE.search(raw) or _LIST_LINE.search(raw) or _URL.search(raw))
    clean = speakable(_FENCE.sub(" ", raw))
    clean = _URL.sub("", clean)
    clean = re.sub(r"\s+", " ", clean).strip()
    sentences = [s.strip() for s in _SENTENCE.split(clean) if s.strip()]
    if not sentences:
        return ""
    if len(clean) <= cap and not had_extra:
        return clean  # short and plain: say all of it
    picked = [sentences[0]]
    for s in sentences[1:]:
        if _RESULT_WORDS.search(s) and len(picked[0]) + len(s) + 1 <= cap:
            picked.append(s)
            break
    spoken = " ".join(picked)
    cut = len(spoken) > cap
    if cut:
        spoken = spoken[: cap - 1].rsplit(" ", 1)[0].rstrip(",;:") + "."
    if had_extra or cut or len(sentences) > len(picked):
        tail = f" The rest is {screen}."
        spoken = spoken.rstrip() + tail if len(spoken) + len(tail) <= cap + len(tail) else spoken
    return spoken


def clip(text: str, cap: int) -> str:
    text = re.sub(r"\s+", " ", speakable(text)).strip()
    if len(text) <= cap:
        return text
    return text[: cap - 1].rsplit(" ", 1)[0].rstrip(",;:") + "."


def parse_done(text: str) -> tuple[str, bool] | None:
    """The kernel's `done` note on a parent's transcript: `Turn ended. Last words: ...`.
    Returns (last words, ok) or None when the text is something else."""
    m = _DONE_PREFIX.match(text.strip())
    if not m:
        return None
    ok = m.group(1) == "ended"
    words = _TRANSCRIPT_NOTE.sub("", text.strip()[m.end():]).strip()
    return words, ok


def speak_name(agent: str) -> str:
    """Agent ids are slugs (`fix-skeptic-test`); say them as words."""
    return re.sub(r"[-_]+", " ", agent)[:40]


class Narrator:
    def __init__(
        self,
        kernel: KernelClient,
        *,
        speak: SpeakHook,
        emit: EmitHook,
        agent: str = "root",
        screen: str = "on your screen",
        model: str | None = None,
        api_key: str | None = None,
        user_talking: Callable[[], bool] = lambda: False,
        arbos_talking: Callable[[], bool] = lambda: False,
        ack: bool = True,
        speak_details: bool = True,
        device: str = "",
    ):
        self.kernel = kernel
        self.speak = speak  # voices one line with the gateway TTS; returns when it has been said
        self.emit = emit  # emit(msg_type, **fields) to the client
        self.agent = agent
        self.screen = screen
        self.model = model
        self.api_key = api_key
        self.user_talking = user_talking
        self.arbos_talking = arbos_talking
        self.device = device  # phone | desktop: written beside `channel` on every message
        self.ack = ack  # say ACK when forwarding an utterance (the speech model's own voice is off)
        self.speak_details = speak_details  # voice more_detail answers here (else the speech model reads them)
        self.queue: asyncio.Queue[Line] = asyncio.Queue()
        self.said: list[Line] = []  # everything spoken or attempted, oldest first
        self.summary: list[str] = []  # rolling summary of the main transcript, one line per event
        self.pending_ask: dict | None = None
        self.turn_running = False
        self._reported: dict[str, float] = {}  # child -> when its report was spoken (dedupe)
        self._task: asyncio.Task | None = None
        self._gen = 0
        self._current: Line | None = None
        self._turn_texts: list[tuple[str, str]] = []  # assistant texts of the open turn, with refs
        self._flush_gen = 0
        self._pending: tuple[str, str] | None = None
        self._pending_handle: asyncio.TimerHandle | None = None
        self._late_until = 0.0
        self.stats = {"heard": 0, "skipped": 0, "interrupted": 0, "details": 0}

    # ------------------------------------------------------------------ lifecycle

    def start(self) -> None:
        self.kernel.listeners.append(self.on_frame)
        self._task = asyncio.create_task(self._speaker(), name="narrator")

    def close(self) -> None:
        if self.on_frame in self.kernel.listeners:
            self.kernel.listeners.remove(self.on_frame)
        self._forward_pending()  # the last words of a call still reach the agent
        if self._task:
            self._task.cancel()

    def interrupted(self) -> None:
        """The caller spoke over us: what was being said is not heard; the queue is dropped."""
        self._gen += 1
        if self._current is not None and self._current.heard is None:
            self._current.heard = False
            self.stats["interrupted"] += 1
        while not self.queue.empty():
            line = self.queue.get_nowait()
            line.heard = False
            self.said.append(line)
            self.stats["skipped"] += 1

    # ------------------------------------------------------------------ the caller

    def user_said_later(self, text: str, *, channel: str = "voice", grace: float = FORWARD_GRACE_S) -> None:
        """A caller utterance goes to the main agent unless, within `grace`, the speech model claims it
        as a question for the narrator (`more_detail`, `agent_status`). Nothing is lost when the
        model stays quiet; a drill-down does not wake the main agent."""
        text = text.strip()
        if not text:
            return
        if self.pending_ask is not None:
            self.user_said(text, channel=channel)
            return
        self._cancel_pending()
        self._pending = (text, channel)
        self._pending_handle = asyncio.get_running_loop().call_later(grace, self._forward_pending)

    def consume_pending(self) -> str | None:
        """The speech model answered the last utterance itself (a tool call): do not forward it."""
        if self._pending is None:
            return None
        text, _ = self._pending
        self._cancel_pending()
        self.summary.append(f"voice (to narrator): {text}")
        return text

    def _forward_pending(self) -> None:
        if self._pending is None:
            return
        text, channel = self._pending
        self._pending = None
        self._pending_handle = None
        if self.ack and self.pending_ask is None:
            self._enqueue(Line("ack", ACK, ref=""))
        self.user_said(text, channel=channel)

    def _cancel_pending(self) -> None:
        if self._pending_handle is not None:
            self._pending_handle.cancel()
        self._pending = None
        self._pending_handle = None

    def user_said(self, text: str, *, channel: str = "voice") -> None:
        """A caller utterance (or a typed line during the call) goes to the main agent as a user
        message. The kernel files it in the inbox with `channel`; a running turn takes it as a steer."""
        text = text.strip()
        if not text:
            return
        if self.pending_ask is not None:
            ask = self.pending_ask
            self.pending_ask = None
            frame = {"type": "answer", "agent": ask.get("agent", self.agent), "text": text}
            if ask.get("id"):
                frame["id"] = ask["id"]
            self.kernel.send(frame)
            self.summary.append(f"answered: {text}")
            return
        self.kernel.send_user(text, self.agent, channel=channel, device=self.device)
        self.summary.append(f"{channel}: {text}")

    # ------------------------------------------------------------------ the kernel

    def on_frame(self, frame: dict) -> None:
        kind = frame.get("type")
        agent = frame.get("agent")
        if kind == "turn" and agent == self.agent:
            self.turn_running = frame.get("state") == "running"
            if self.turn_running:
                # The next turn starts before the last one's flush fired (a typed line right
                # behind the spoken one): speak the last turn now, then collect the new one. The
                # last turn's whole text can still trail its `idle` by a moment; for a short
                # while an assistant event is taken as that turn's, not the new one's.
                self._flush_gen += 1
                if self._turn_texts:
                    self._speak_turn()
                self._late_until = time.monotonic() + 0.8
            if not self.turn_running:
                # The whole assistant text can follow `idle` by a moment; gather it, then speak once.
                self._flush_gen += 1
                asyncio.get_running_loop().call_later(0.7, self._flush_turn, self._flush_gen)
        elif kind == "turn" and agent:
            # A child's turn ended. The kernel's `done` file reaches us as a `say` on the parent a
            # moment later; if it does not (an older kernel), report from what the child said.
            state = self.kernel.agents.get(agent)
            if frame.get("state") == "idle" and state is not None and state.parent == self.agent:
                asyncio.get_running_loop().call_later(2.5, self._late_report, agent)
        elif kind == "event" and agent == self.agent:
            event = frame.get("event") or {}
            ek = event.get("kind")
            text = str(event.get("text") or "")
            if ek == "assistant" and text.strip():
                # One highlight per turn, from its last words: a turn with tool calls has several
                # assistant steps and only the last one states the result.
                squashed = "".join(text.split())
                if self.turn_running and time.monotonic() < self._late_until and not self._turn_texts:
                    line = highlight(text, screen=self.screen)  # the previous turn's final words
                    if line:
                        self.summary.append(f"arbos: {line}")
                        self._enqueue(Line("highlight", line, ref=_ref(frame)))
                    return
                if not any(squashed == "".join(t.split()) for t, _ in self._turn_texts):
                    self._turn_texts.append((text, _ref(frame)))
                if not self.turn_running and self._flush_gen:
                    self._flush_gen += 1
                    asyncio.get_running_loop().call_later(0.7, self._flush_turn, self._flush_gen)
            elif ek == "say" and text.strip():
                child = str(event.get("from") or "")
                done = parse_done(text)
                if done is not None:
                    words, ok = done
                    self._report(child, words, ok, ref=_ref(frame))
                else:
                    self.summary.append(f"{child} says: {clip(text, 120)}")
            elif ek == "notice" and event.get("failed") and text.strip():
                self._enqueue(Line("error", "Something failed: " + clip(text, ERROR_CAP), ref=_ref(frame)))
            elif ek == "user" and text.strip():
                pass  # the caller's own words; never read back
        elif kind == "ask" and agent == self.agent:
            self.pending_ask = frame
            question = clip(str(frame.get("question") or ""), REPORT_CAP)
            options = [str(o) for o in frame.get("options") or []]
            spoken = f"Arbos asks: {question}"
            if options:
                spoken += " Options: " + ", ".join(options) + "."
            self.summary.append(f"ask: {question}")
            self._enqueue(Line("ask", spoken, ref=f"ask:{frame.get('id') or ''}"))
        elif kind == "error":
            detail = clip(str(frame.get("detail") or ""), ERROR_CAP)
            if detail:
                self._enqueue(Line("error", "Something failed: " + detail))

    def _flush_turn(self, gen: int) -> None:
        if gen != self._flush_gen or self.turn_running or not self._turn_texts:
            return
        self._speak_turn()

    def _speak_turn(self) -> None:
        text, ref = self._turn_texts[-1]
        self._turn_texts.clear()
        line = highlight(text, screen=self.screen)
        if line:
            self.summary.append(f"arbos: {line}")
            self._enqueue(Line("highlight", line, ref=ref))

    def child_finished(self, child: str, words: str, *, ok: bool = True) -> None:
        """The tool bridge saw a dispatched agent end (kernels without `done` files)."""
        self._report(child, words, ok, ref=f"agent:{child}")

    def _late_report(self, child: str) -> None:
        if child in self._reported:
            return
        state = self.kernel.agents.get(child)
        if state is None or state.running:
            return
        words = state.says[-1] if state.says else state.assistant.strip()
        self._report(child, words, True, ref=f"agent:{child}")

    def _report(self, child: str, words: str, ok: bool, *, ref: str) -> None:
        now = time.monotonic()
        if now - self._reported.get(child, -1e9) < 5.0:
            return
        self._reported[child] = now
        name = speak_name(child) or "an agent"
        body = clip(words, REPORT_CAP) if words else "no report"
        verb = "is done" if ok else "ended badly"
        self.summary.append(f"{child} {verb}: {body}")
        self._enqueue(Line("report", f"{name} {verb}: {body}", ref=ref))

    def _enqueue(self, line: Line) -> None:
        if line.kind == "ack" and (not self.queue.empty() or self._current is not None):
            return  # something is being said already; that is acknowledgement enough
        if self.queue.qsize() >= QUEUE_CAP:
            dropped = self.queue.get_nowait()
            dropped.heard = False
            self.said.append(dropped)
            self.stats["skipped"] += 1
            line.text = line.text.rstrip(".") + f", and more {self.screen}."
        self.queue.put_nowait(line)

    # ------------------------------------------------------------------ speaking

    async def _speaker(self) -> None:
        while True:
            line = await self.queue.get()
            # Two reports close together become one line.
            if line.kind == "report":
                await asyncio.sleep(0.3)
                extra = []
                while not self.queue.empty() and self.queue._queue[0].kind == "report":  # noqa: SLF001
                    extra.append(self.queue.get_nowait())
                if extra:
                    names = [line] + extra
                    line = Line(
                        "report",
                        f"{len(names)} agents finished: " + "; ".join(l.text for l in names),
                        ref=",".join(l.ref for l in names),
                    )
            gen = self._gen
            # Never talk over the caller; wait for the model to finish its own sentence.
            waited = 0.0
            while (self.user_talking() or self.arbos_talking()) and waited < YIELD_TO_USER_S:
                await asyncio.sleep(0.1)
                waited += 0.1
            if gen != self._gen:  # interrupted while waiting
                line.heard = False
                self.said.append(line)
                continue
            self._current = line
            self.emit(P.NARRATOR_SAY, text=line.text, kind=line.kind, ref=line.ref)
            try:
                await self.speak(line.text)
            except Exception:
                log.exception("narrator could not speak")
            if line.heard is None:
                line.heard = True
                self.stats["heard"] += 1
            self.said.append(line)
            self._current = None

    # ------------------------------------------------------------------ drill-down

    async def more_detail(self, question: str) -> str:
        """Answer from the record, in a few sentences. Escalates to the main agent only when the
        record does not hold the answer."""
        self.stats["details"] += 1
        question = question.strip()
        if _wants_repeat(question):
            last = next((l for l in reversed(self.said) if l.kind in ("highlight", "report", "detail")), None)
            return last.text if last else "I have not said anything yet."
        events = await self.kernel.transcript_tail(self.agent)
        children = [a for a in self.kernel.agents.values() if a.parent == self.agent]
        child_events: dict[str, list[dict]] = {}
        for child in children[-3:]:
            child_events[child.name] = await self.kernel.transcript_tail(child.name, bytes_=120_000)
        facts = _facts(events, child_events)
        if not facts:
            self.kernel.send_user(question, self.agent, channel="voice", device=self.device)
            return "The record does not say. I have asked the main agent; I will tell you when it answers."
        answer = _extractive(question, facts)
        if self.model and self.api_key:
            try:
                answer = await self._summarise(question, facts)
            except Exception as exc:
                log.warning("narrator model failed, using the extractive answer: %s", exc)
        answer = clip(answer, DETAIL_CAP)
        if self.speak_details:
            self._enqueue(Line("detail", answer, ref="transcript"))
        else:
            line = Line("detail", answer, ref="transcript")
            line.heard = True
            self.said.append(line)
            self.emit(P.NARRATOR_SAY, text=answer, kind="detail", ref="transcript")
        return answer

    async def _summarise(self, question: str, facts: list[str]) -> str:
        prompt = (
            "You narrate a software agent's work over the phone. Answer the question from the record below in at "
            "most three short plain sentences. No markdown, no lists, no code. If the record does not answer it, "
            "say what the record does say and that the rest is on the screen.\n\nQuestion: "
            + question
            + "\n\nRecord:\n"
            + "\n".join(facts[-40:])
        )
        async with httpx.AsyncClient(timeout=httpx.Timeout(30.0, connect=10.0)) as client:
            resp = await client.post(
                "https://openrouter.ai/api/v1/chat/completions",
                json={"model": self.model, "messages": [{"role": "user", "content": prompt}], "max_tokens": 200},
                headers={"Authorization": f"Bearer {self.api_key}", "X-Title": "Arbos voice narrator"},
            )
            resp.raise_for_status()
            data = resp.json()
        return str(data["choices"][0]["message"]["content"]).strip()

    def status_text(self) -> str:
        parts = [self.kernel.status_text()]
        if self.summary:
            parts.append("Most recently: " + clip(self.summary[-1], 160))
        return " ".join(parts)


# ---------------------------------------------------------------------- record reading


def _ref(frame: dict) -> str:
    seq = (frame.get("event") or {}).get("seq")
    return f"transcript:{seq}" if seq else "transcript"


def _wants_repeat(question: str) -> bool:
    q = question.lower()
    return any(p in q for p in ("say that again", "repeat that", "what did you say", "come again", "once more"))


def _facts(root: list[dict], children: dict[str, list[dict]]) -> list[str]:
    """The lines of the record that carry an answer: tool errors, failed notices, agents' last words,
    the last assistant texts, and the tail of tool bodies (not the whole output)."""
    out: list[str] = []

    def take(agent: str, events: list[dict]) -> None:
        for e in events[-80:]:
            kind = e.get("kind")
            if kind == "assistant" and e.get("text"):
                out.append(f"[{agent}] said: {_flat(e['text'], 400)}")
            elif kind == "say" and e.get("text"):
                out.append(f"[{agent}] {e.get('from', '?')} said: {_flat(e['text'], 400)}")
            elif kind == "notice" and e.get("text"):
                out.append(f"[{agent}] notice{' (failed)' if e.get('failed') else ''}: {_flat(e['text'], 300)}")
            elif kind == "tool":
                name = e.get("name", "tool")
                if e.get("error"):
                    out.append(f"[{agent}] tool {name} failed: {_flat(str(e['error']), 300)}")
                body = e.get("body")
                if isinstance(body, str) and body.strip():
                    lines = [l for l in body.splitlines() if l.strip()]
                    interesting = [l for l in lines if re.search(r"error|fail|assert|panic|exception|Traceback|exit", l, re.I)]
                    tail = interesting[-3:] if interesting else lines[-2:]
                    if tail:
                        out.append(f"[{agent}] tool {name} output: " + " | ".join(_flat(l, 160) for l in tail))
            elif kind == "ask" and e.get("question"):
                out.append(f"[{agent}] asked: {_flat(e['question'], 200)}")

    for name, events in children.items():
        take(name, events)
    take("root", root)
    return out


def _flat(text: str, cap: int) -> str:
    text = re.sub(r"\s+", " ", speakable(_FENCE.sub(" code omitted ", text))).strip()
    return text if len(text) <= cap else text[: cap - 1] + "…"


def _extractive(question: str, facts: list[str]) -> str:
    """No model: pick the facts that share words with the question, else the most telling recent ones."""
    words = {w for w in re.findall(r"[a-z]{4,}", question.lower())} - {"exactly", "what", "which", "about", "does", "did", "there", "that", "this", "with"}
    scored: list[tuple[int, int, str]] = []
    for i, f in enumerate(facts):
        low = f.lower()
        score = sum(1 for w in words if w in low)
        if re.search(r"fail|error|because|assert|panic", low):
            score += 2
        if "said:" in low and "[root]" in low:
            score += 1
        scored.append((score, i, f))
    scored.sort(key=lambda t: (-t[0], -t[1]))
    picked = [f for s, _, f in scored[:3] if s > 0] or [f for _, _, f in scored[:2]]
    picked.sort(key=lambda f: facts.index(f))
    body = " ".join(re.sub(r"^\[[^\]]+\]\s*(?:said:\s*)?", "", f) for f in picked)
    return "From the record: " + body


def openrouter_key() -> str | None:
    return os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENROUTER")
