"""#1860 — Jacob's rule: the main voice agent is the only speaker on a call. A job that finishes
during the call produces no second speaker; its result goes into the voice agent's context and
the voice agent says a short summary at the next pause, never over the person.

    source .venv/bin/activate && python -m pytest tests/test_updates.py -q
"""

from __future__ import annotations

import asyncio

import pytest

from arbos_voice.updates import PAUSE_S, CallUpdates, Update, context_item, note_for

pytest_plugins = ("pytest_asyncio",)


class FakeCall:
    """What the worker hands CallUpdates: a context sink, ONE speaker, and the room state."""

    def __init__(self):
        self.context: list[str] = []
        self.spoken: list[str] = []
        self.interrupts = 0
        self.user_speaking = False
        self.agent_speaking = False
        self.leaving = False
        self.items: list[dict] = []

    async def fetch(self, after: int, wait: int) -> dict:
        items = [i for i in self.items if i["seq"] > after]
        if not items:
            await asyncio.sleep(0.01)
        return {"items": items, "seq": max([i["seq"] for i in self.items] or [after]), "live": not self.leaving}

    def add_context(self, text: str) -> None:
        self.context.append(text)

    async def speak(self, instructions: str) -> None:
        self.agent_speaking = True
        self.spoken.append(instructions)
        await asyncio.sleep(0.01)
        self.agent_speaking = False

    def quiet(self) -> bool:
        return not self.user_speaking and not self.agent_speaking

    def updates(self) -> CallUpdates:
        return CallUpdates(
            agent_name="Bubble",
            fetch=self.fetch,
            add_context=self.add_context,
            speak=self.speak,
            quiet=self.quiet,
            leaving=lambda: self.leaving,
        )


def job_done(message_id: str = "m1", kind: str = "job") -> Update:
    return Update(
        kind=kind,
        message_id=message_id,
        text="Pizza Tomate delivers via WhatsApp +506 8992 4888, open until 10 p.m.; Muzza and Amici also deliver.",
        sender={"kind": "agent", "name": "Bubble", "handle": "bubble"},
    )


@pytest.mark.asyncio
async def test_a_job_completion_during_a_call_is_one_voice_and_lands_in_context(monkeypatch):
    monkeypatch.setattr("arbos_voice.updates.PAUSE_S", 0.05)
    call = FakeCall()
    u = call.updates()
    assert await u.handle(job_done())
    # The result is in the model's context at once …
    assert len(call.context) == 1
    assert call.context[0].startswith("[Chat] A background job of yours finished")
    assert "Pizza Tomate" in call.context[0]
    await asyncio.sleep(0.2)
    # … and the voice agent (the only speaker) said one short summary at the pause.
    assert len(call.spoken) == 1
    assert "one or two short spoken sentences" in call.spoken[0]
    assert "Do not read tables, links or phone numbers aloud" in call.spoken[0]
    assert call.interrupts == 0
    assert u.spoken == 1


@pytest.mark.asyncio
async def test_the_summary_waits_for_the_person_to_finish_and_never_interrupts(monkeypatch):
    monkeypatch.setattr("arbos_voice.updates.PAUSE_S", 0.05)
    call = FakeCall()
    call.user_speaking = True
    u = call.updates()
    await u.handle(job_done(kind="result"))
    await asyncio.sleep(0.2)
    assert call.spoken == [], "the person is talking: nothing is said over them"
    assert call.context, "but the result is already in context for the next turn"
    call.user_speaking = False
    await asyncio.sleep(0.2)
    assert len(call.spoken) == 1, "said once the person paused"
    assert call.interrupts == 0


@pytest.mark.asyncio
async def test_no_pause_means_no_second_speaker_the_next_turn_has_it(monkeypatch):
    monkeypatch.setattr("arbos_voice.updates.SPEAK_WAIT_S", 0.15)
    monkeypatch.setattr("arbos_voice.updates.PAUSE_S", 0.05)
    call = FakeCall()
    call.user_speaking = True
    u = call.updates()
    await u.handle(job_done())
    await asyncio.sleep(0.4)
    assert call.spoken == []
    assert len(call.context) == 1
    assert u.spoken == 0


@pytest.mark.asyncio
async def test_status_lines_and_chat_messages_are_context_only():
    call = FakeCall()
    u = call.updates()
    await u.handle(Update(kind="quick", message_id="q1", text="Still checking Santa Teresa.", sender={"kind": "agent"}))
    await u.handle(Update(kind="message", message_id="h1", text="Did you call me?", sender={"kind": "user", "name": "Dawizzaa"}))
    await asyncio.sleep(0.1)
    assert call.spoken == []
    assert [c.split("\n")[0] for c in call.context] == [
        "[Chat] Your agent posted a status line in the chat:",
        "[Chat] Dawizzaa wrote in the chat:",
    ]


@pytest.mark.asyncio
async def test_the_same_bubble_from_the_tool_and_the_inbox_is_handled_once(monkeypatch):
    monkeypatch.setattr("arbos_voice.updates.PAUSE_S", 0.05)
    call = FakeCall()
    u = call.updates()
    assert u.mark_seen("m1"), "the ask_agent result came back with this message id"
    assert not await u.handle(job_done("m1", kind="result")), "the inbox copy is a duplicate"
    assert await u.handle(job_done("m2", kind="result"))
    await asyncio.sleep(0.2)
    assert len(call.context) == 1
    assert len(call.spoken) == 1


@pytest.mark.asyncio
async def test_the_poll_loop_feeds_updates_and_stops_when_the_call_ends(monkeypatch):
    monkeypatch.setattr("arbos_voice.updates.PAUSE_S", 0.05)
    call = FakeCall()
    u = call.updates()
    u.start()
    call.items.append({"seq": 1, "kind": "job", "message_id": "j1", "text": "Found three options.", "sender": {"kind": "agent"}})
    await asyncio.sleep(0.3)
    assert len(call.context) == 1 and len(call.spoken) == 1
    assert u.seq == 1
    call.leaving = True
    await asyncio.sleep(0.1)
    await u.stop()
    assert PAUSE_S > 0


def test_notes_and_context_items_are_plain_user_text():
    n = note_for(job_done(), "Bubble")
    assert n.startswith("[Chat]") and "WhatsApp" in n
    item = context_item(n)
    assert item["type"] == "conversation.item.create"
    assert item["item"]["role"] == "user"
    assert item["item"]["content"][0]["type"] == "input_text"
    assert "response.create" not in str(item)
