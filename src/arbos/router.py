"""Front-line routing agent that runs upstream of the heavy chat worker.

Most user messages either map cleanly onto an Arbos slash command (a cron
add, a workspace change, a restart, an update, a /plan kickoff) or are
small enough that the router can answer them directly from chat history
without spawning a fresh worker. We want the routing decision to be made
by an LLM that *has chat-history context* -- the prior router was a
stateless OpenAI ``gpt-4o-mini`` function-call hop with no memory of the
topic, so e.g. "do that again every 5 mins" routed badly.

The :class:`SmartRouter` here is a thin wrapper around a ``cursor-agent``
invocation:

* run in **agent mode** (so the router can read files / run small shell
  checks / inspect state if it helps the routing decision);
* with ``--resume <router_session_id>`` so the per-topic SmartRouter has
  full chat-history context (separate session from the worker's chat,
  see :mod:`session_store`);
* given a tight system prompt that teaches the slash-command protocol
  the agent layer parses to dispatch operational ops back into Arbos's
  existing handlers;
* free to answer the user directly in plain text when the question only
  needs recall ("what did we last decide?") rather than action.

Output protocol (parsed by :func:`SmartRouter.parse_slash`):

* The agent's final answer's last non-empty line, with surrounding
  markdown / backticks stripped, may be one of::

      /delegate
      /restart
      /update
      /workspace <path>
      /cron <mins> <prompt...>
      /plan <request...>

  When matched, the agent layer dispatches via :class:`RouterContext`'s
  bound ``do_*`` callables, which already wrap the existing slash
  handlers. The leading text (the agent's reasoning) is shown to the
  user as the routing bubble's recap.

* Otherwise the router is treated as having answered directly: the
  recap text becomes the user-visible answer and no worker is spawned.

The actual cursor-agent run (Bubble + Supervisor + CursorRunner wiring,
session id persistence) lives in :mod:`agent` -- this module owns only
the prompt, the parser, and the dispatch glue.
"""

from __future__ import annotations

import enum
import logging
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Awaitable, Callable, Optional

logger = logging.getLogger(__name__)


# Special slash that signals "delegate to the heavy chat worker". Not a
# real Arbos slash command (the literal /delegate is not user-facing);
# the agent layer recognises it as the pass-through marker.
DELEGATE_SLASH = "/delegate"


class RouterOutcome(enum.Enum):
    """Result of dispatching a SmartRouter decision.

    * ``HANDLED`` -- a slash command was parsed and its ``do_*`` handler
      already drove its own bubble. The caller MUST NOT spawn the worker.
    * ``DELEGATE`` -- no slash matched, OR ``/delegate`` was emitted, OR
      any failure path. The caller continues with the worker dispatch
      against the same bubble. Failing open keeps the agent responsive
      even if the router itself misbehaves.
    * ``ANSWERED`` -- no slash, but the router's recap text is the
      user-visible answer. The caller posts it as a summary bubble and
      does NOT spawn the worker.
    """

    HANDLED = "handled"
    DELEGATE = "delegate"
    ANSWERED = "answered"


@dataclass
class RouterContext:
    """Per-message handles the SmartRouter's dispatch needs.

    Constructed fresh for each :meth:`SmartRouter.dispatch` call by the
    caller in ``agent.py``; the SmartRouter itself is stateless across
    calls. The bound ``do_*`` callables wrap existing :class:`CursorAgent`
    handlers so the router never reimplements business logic -- it just
    parses + routes. Each ``do_*`` owns its own user-facing bubble
    lifecycle.
    """

    machine_name: str
    topic_id: int
    workspace: Path
    do_workspace: Callable[[Path], Awaitable[None]]
    do_restart_with_confirm: Callable[[], Awaitable[None]]
    do_update_with_confirm: Callable[[], Awaitable[None]]
    do_add_cron: Callable[[int, str], Awaitable[None]]
    do_plan: Callable[[str], Awaitable[None]]
    do_say: Callable[[str], Awaitable[None]]


@dataclass
class RouterDecision:
    """Parsed SmartRouter output for one user message.

    ``raw_final`` is the agent's full final answer (verbatim). ``recap``
    is the same text with the trailing slash command stripped, suitable
    for posting as the routing bubble's user-visible recap.
    ``slash`` is ``(command, args)`` if the last non-empty line parsed
    as one of the recognised commands; ``None`` otherwise.
    """

    raw_final: str
    recap: str
    slash: Optional[tuple[str, str]] = None


# Recognised slash commands the router may emit. Mapped to a short
# spec describing args: ``"none"`` for parameterless, ``"path"`` for
# a single path arg, ``"int+rest"`` for ``<mins> <prompt>``,
# ``"rest"`` for a single free-form trailing string. Used both by the
# parser (which validates shape) and the system prompt builder (which
# renders the grammar for the LLM).
_SLASH_SPECS: dict[str, str] = {
    "/delegate": "none",
    "/restart":  "none",
    "/update":   "none",
    "/workspace": "path",
    "/cron":     "int+rest",
    "/plan":     "rest",
}


# Strip surrounding markdown decorations off a candidate slash line.
# We trim backticks, asterisks, leading list-bullet glyphs, and trailing
# punctuation; anything else (e.g. random emoji) just won't parse.
_DECOR_STRIP = re.compile(r"^[`*_>\-\s]+|[`*_\s.;,]+$")

_INT_RE = re.compile(r"^\d+$")


def _normalise_slash_line(line: str) -> str:
    """Strip markdown / list decorations off a candidate final-line slash."""
    s = line.strip()
    s = _DECOR_STRIP.sub("", s)
    return s.strip()


def _parse_one_slash(line: str) -> Optional[tuple[str, str]]:
    """Try to parse ``line`` as a single slash command. Returns ``(cmd, args)``."""
    s = _normalise_slash_line(line)
    if not s.startswith("/"):
        return None
    head, _, tail = s.partition(" ")
    cmd = head.lower()
    spec = _SLASH_SPECS.get(cmd)
    if spec is None:
        return None
    args = tail.strip()
    if spec == "none":
        # Parameterless; tolerate trailing junk by ignoring it. (Models
        # sometimes append a period or a parenthetical note.)
        return (cmd, "")
    if spec == "path":
        if not args:
            logger.info("router: %s missing path arg in %r", cmd, line)
            return None
        return (cmd, args)
    if spec == "rest":
        if not args:
            logger.info("router: %s missing trailing arg in %r", cmd, line)
            return None
        return (cmd, args)
    if spec == "int+rest":
        n_head, _, n_tail = args.partition(" ")
        if not _INT_RE.match(n_head) or not n_tail.strip():
            logger.info("router: %s expected '<int> <prompt>', got %r", cmd, args)
            return None
        return (cmd, args)
    return None  # unreachable; defensive


_ROUTER_SYSTEM_PROMPT = """\
You are the SmartRouter for Arbos on machine `{machine_name}`, topic \
`{topic_id}` (workspace `{workspace}`).

Every message in this topic flows through you. You have this topic's \
prior chat history (this is a resumed cursor-agent session) so you can \
resolve "do that again", "the previous one", "what did we just \
decide", etc. You ALSO have a working filesystem + shell, so for small \
fact-finding questions you can just answer them yourself.

You operate in three modes (mutually exclusive per message):

(A) ANSWER. The user asked a small question that can be resolved by \
recall, by reading a file in the workspace or in `~/.arbos/`, or by a \
short shell command. Answer in plain English. Do NOT emit any slash \
command in this case -- your text IS the user-visible answer.

  Examples worth answering yourself:
    - "do we have active crons?" -> read `~/.arbos/crons.json`, list \
      them in 1-3 lines.
    - "what file did we touch last?" -> recall from chat history.
    - "is doppler set up?" -> `which doppler` / quick check.
    - greetings, clarifications, "what did we just decide?".

(B) DISPATCH. The user is asking for an Arbos operational action. End \
your reply with EXACTLY ONE slash command on its own final line:

    /restart
    /update
    /workspace <path>
    /cron <mins> <prompt...>
    /plan <request...>

  Meanings:
  - `/restart` -- pm2-reload arbos on this machine. Use for "restart \
    yourself", "reload arbos". A Confirm/Cancel button is shown.
  - `/update` -- git fetch + hard-reset to origin/main + reinstall + \
    pm2 reload. Use for "update from main", "pull and restart". \
    Confirm/Cancel is shown.
  - `/workspace <path>` -- change THIS topic's cursor-agent cwd. Use \
    for "change workspace to ~/foo", "cd into /tmp/x". Pass the path \
    verbatim; the dispatcher validates it.
  - `/cron <mins> <prompt>` -- schedule a recurring prompt every \
    <mins> minutes in this topic. Parse the period from natural \
    language ("every 5 mins" -> 5, "every hour" -> 60, "every 2h" -> \
    120). Pass the rest of the user request verbatim as <prompt>.
  - `/plan <request>` -- run the 2-phase plan-then-implement flow. \
    Use when the user explicitly asks to plan first OR when the \
    request is large/architectural. Pass the user's request verbatim.

  Before the slash command you may write 1-3 short sentences \
explaining the decision (becomes the bubble's recap). Plain English: \
"Scheduling a 5-minute disk check." Not mechanical: not "I will call \
/cron with mins=5 ...".

(C) DELEGATE. The user wants substantive coding, multi-step \
investigation, file editing across many places, or anything that will \
take more than a quick read. End your reply with `/delegate` on its \
own final line. The heavy chat worker takes over from there.

  Use /delegate liberally for real work: misclassifying a real task as \
something cheap is worse than over-delegating. The worker has the \
SAME chat history (separate session, but mirrors the user's messages) \
so it won't lose context.

Hard rules:
- The slash command, if present, MUST be on its own final line, with \
  NOTHING after it -- no trailing period, quote, parenthesis, or \
  markdown fence. The dispatcher does an exact line match.
- Pick AT MOST ONE slash command. If multiple things need doing, \
  emit `/delegate` and let the worker sequence them.
- Do NOT emit a slash command for things you already answered (mode \
  A). Only one of A / B / C per message.
- Do NOT use shell, file edits, or other write tools to do work that \
  has a slash command -- emit the slash command, the dispatcher will \
  execute it correctly. (Reading state to inform an answer is fine.)
- Do NOT ask clarifying questions when the answer or arg is obvious \
  from the message + history. When truly ambiguous, prefer \
  `/delegate` over a clarification round-trip.
- Keep mode-A answers tight (a few lines max). Long-form output is the \
  worker's job; emit `/delegate` if the user wants depth.
"""


class SmartRouter:
    """Stateless prompt + parser for one routing decision.

    The actual cursor-agent invocation (Bubble + Supervisor wiring,
    session-id persistence) lives in :mod:`agent`; this class just
    exposes the system prompt template, the slash-command parser, and
    the dispatch glue that turns a parsed ``(cmd, args)`` pair into the
    correct ``RouterContext.do_*`` call.
    """

    def __init__(self, *, enabled: bool = True) -> None:
        self._enabled = bool(enabled)

    @property
    def enabled(self) -> bool:
        """If False, the agent layer skips the router entirely (always delegates)."""
        return self._enabled

    def build_system_prompt(self, ctx: RouterContext) -> str:
        """Render the SmartRouter system preamble for ``ctx``."""
        return _ROUTER_SYSTEM_PROMPT.format(
            machine_name=ctx.machine_name,
            topic_id=ctx.topic_id,
            workspace=str(ctx.workspace),
        )

    @staticmethod
    def parse_decision(final_text: str) -> RouterDecision:
        """Split ``final_text`` into ``(recap, slash)``.

        Walks the final answer bottom-up: the last non-empty line is
        considered as a candidate slash. If it parses, ``slash`` is
        ``(cmd, args)`` and ``recap`` is everything above that line
        (with trailing whitespace trimmed). If it doesn't, ``slash``
        is ``None`` and ``recap`` is the whole text.
        """
        text = (final_text or "").rstrip()
        if not text:
            return RouterDecision(raw_final="", recap="", slash=None)
        lines = text.splitlines()
        # Find last non-empty line index.
        last_idx = -1
        for i in range(len(lines) - 1, -1, -1):
            if lines[i].strip():
                last_idx = i
                break
        if last_idx < 0:
            return RouterDecision(raw_final=text, recap=text, slash=None)
        candidate = lines[last_idx]
        slash = _parse_one_slash(candidate)
        if slash is None:
            return RouterDecision(raw_final=text, recap=text, slash=None)
        recap = "\n".join(lines[:last_idx]).rstrip()
        return RouterDecision(raw_final=text, recap=recap, slash=slash)

    async def dispatch(
        self,
        cmd: str,
        args: str,
        ctx: RouterContext,
    ) -> RouterOutcome:
        """Execute one parsed slash via ``ctx``'s ``do_*`` handlers.

        Returns ``DELEGATE`` for ``/delegate`` (caller spawns the worker)
        and ``HANDLED`` for every other recognised slash. Argument-shape
        errors are surfaced to the user via ``do_say`` and counted as
        ``HANDLED`` so we never *also* spawn the worker on the same
        message. Unknown commands are logged and treated as ``DELEGATE``.
        """
        cmd = cmd.lower()
        if cmd == DELEGATE_SLASH:
            return RouterOutcome.DELEGATE
        try:
            if cmd == "/restart":
                await ctx.do_restart_with_confirm()
                return RouterOutcome.HANDLED
            if cmd == "/update":
                await ctx.do_update_with_confirm()
                return RouterOutcome.HANDLED
            if cmd == "/workspace":
                raw = (args or "").strip()
                if not raw:
                    await ctx.do_say("/workspace: missing path argument")
                    return RouterOutcome.HANDLED
                try:
                    target = Path(raw).expanduser().resolve()
                except (OSError, RuntimeError) as exc:
                    await ctx.do_say(f"could not resolve {raw!r}: {exc}")
                    return RouterOutcome.HANDLED
                await ctx.do_workspace(target)
                return RouterOutcome.HANDLED
            if cmd == "/cron":
                n_head, _, n_tail = (args or "").strip().partition(" ")
                try:
                    mins = int(n_head)
                except ValueError:
                    await ctx.do_say(
                        f"/cron: 'mins' must be an integer, got {n_head!r}"
                    )
                    return RouterOutcome.HANDLED
                body = n_tail.strip()
                if mins < 1 or not body:
                    await ctx.do_say("/cron: usage `/cron <mins> <prompt>`")
                    return RouterOutcome.HANDLED
                await ctx.do_add_cron(mins, body)
                return RouterOutcome.HANDLED
            if cmd == "/plan":
                body = (args or "").strip()
                if not body:
                    await ctx.do_say("/plan: missing request")
                    return RouterOutcome.HANDLED
                await ctx.do_plan(body)
                return RouterOutcome.HANDLED
        except Exception as exc:
            logger.exception("router: dispatch of %s crashed", cmd)
            try:
                await ctx.do_say(f"router dispatch of {cmd} crashed: {exc}")
            except Exception:
                logger.exception("router: do_say crashed during dispatch error")
            return RouterOutcome.HANDLED
        logger.warning("router: unrecognised slash %r; delegating", cmd)
        return RouterOutcome.DELEGATE
