"""Who answers a spoken turn in call mode: the kernel (the project) or the speech model (small talk)."""

from __future__ import annotations

import re

_SMALL_TALK = re.compile(
    r"^(hi|high|hello|hey|yo|hiya|good (morning|afternoon|evening|night)|thanks?( you| a lot)?|thank you|cheers|"
    r"ok(ay)?|yes|yeah|yep|no|nope|sure|got it|right|cool|great|nice|perfect|bye|goodbye|see you|later|"
    r"how are you( doing)?|how's it going|what's up|can you hear me|are you (there|listening)|hello\??|testing|"
    r"never ?mind|stop|wait|hold on|one (moment|second|sec)|say that again|repeat that|pardon|sorry|"
    r"who are you|what are you|what is your name|tell me a joke)\b",
    re.I,
)
_PROJECT_WORDS = re.compile(
    r"\b(we|our|project|work(ing)?|agent|agents|task|code|repo|file|files|pr|pull request|branch|test|tests|"
    r"build|deploy|bug|fix|status|progress|plan|todo|next|done|finished|running|kernel|send|dispatch|"
    r"spawn|write|create|make|run|check|look|find|search|open|read|update|change|remember|note|"
    r"what|which|where|when|why|how much|how many)\b",
    re.I,
)


def is_small_talk(text: str) -> bool:
    """True for greetings, acknowledgements and the like, which need no project knowledge.

    Anything mentioning work, agents, code or an action goes to the kernel even if short.
    Long utterances go to the kernel regardless.
    """
    words = text.strip().strip(".,!?").split()
    if not words:
        return True
    if _PROJECT_WORDS.search(text):
        return False
    if len(words) > 7:
        return False
    return bool(_SMALL_TALK.match(text.strip()))
