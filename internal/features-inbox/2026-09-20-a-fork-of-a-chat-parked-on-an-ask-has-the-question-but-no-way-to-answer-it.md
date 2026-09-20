# A fork of a chat parked on an ask has the question in its transcript but nothing to answer

**Seen:** cycle 87 of the desktop symmetry loop, place `/tmp/rl85-proj`, kernel `arbos-kernel 0.2.0 e504bf2f48ad protocol 1`. Still: `media/cursor-reference/cycle-87/rewind/fork-of-a-parked-ask-no-card.png`.

**What happened.** `root` asked *Which option do you prefer?* (alpha / beta) and parked; `agents/root/waiting/ask-tool_ask_KSTbMry05dnmEhgrjFbo.toml` holds the question. The desktop forked the chat (`arbos_core::files::fork_chat`). The copy, `chat-1789885635756`, has the transcript whole — the `ask` tool line and *Waiting for your answer* — but no `waiting/` dir, so `pending_asks` offers nothing for it at attach and the window draws the line with no card under it. The original keeps its card (verified after a relaunch: `asks_replayed count=2` — root and an unrelated chat).

**Ask.** `fork_chat` should copy `waiting/` with the transcript (the ask id can stay the same — `answer(agent, ask_id)` is per agent), so a fork made while a question is open can be answered on its own, as the original can. Alternatively, the fork's transcript could close the question — an `answer` line saying the fork left it — so the *Waiting for your answer* line is not there to read. Copying is what Cursor's fork does: the conversation, question included, is the person's to continue either way.

**Desktop side.** Nothing until the kernel decides: the window draws the card the kernel offers at attach, and the fork is offered none. A desktop-made card would answer into a waiting file that does not exist.
