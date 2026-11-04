"""Offline tests for the resourcefulness rubric (AF-792). No relay, no box, no model.

`python3 -m pytest evals/long-tasks/test_resourcefulness.py` (or `-m unittest`). No CI job runs
Python in this repository, so this is a local check.

The three shaped cases are the comparison agent's affine.io turn, and the two observed Arbos
failures. Where the real transcripts are not in evidence the case carries only the behaviour that
is, and `test_an_unrecovered_transcript_scores_na_not_zero` pins that an absent field reads as `n/a`
rather than as a failing score.
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lib import driver as dr  # noqa: E402
from lib import resourcefulness as rf  # noqa: E402


def call(name, target=None, error=None, output_empty=False):
    return {"name": name, "target": target, "error": error, "output_empty": output_empty}


class TargetRecording(unittest.TestCase):
    """`session_metrics` has to hand the rubric a target per call; names alone hide a re-fetch."""

    def session(self, prompt, calls):
        entries = [{"type": "message", "message": {"role": "user", "content": prompt}}]
        for i, (name, args, err) in enumerate(calls):
            entries.append({"type": "message", "message": {
                "role": "assistant",
                "content": [{"type": "toolCall", "name": name, "id": f"c{i}", "input": args}],
            }})
            if err is not None:
                entries.append({"type": "message", "message": {
                    "role": "toolResult", "toolCallId": f"c{i}", "isError": True, "content": err,
                }})
        return entries

    def test_targets_and_errors_line_up_with_the_names(self):
        m = dr.session_metrics(self.session("read it", [
            ("web_fetch", {"url": "https://affine.io/"}, None),
            ("bash", {"command": "curl -s https://affine.io/"}, "curl: (35) SSL connect error"),
        ]), "read it")
        self.assertEqual(m["tool_names"], ["web_fetch", "bash"])
        self.assertEqual(m["tool_targets"], ["https://affine.io/", "curl -s https://affine.io/"])
        self.assertEqual(m["tool_results"][0], None)
        self.assertIn("(35)", m["tool_results"][1])
        self.assertEqual(m["tool_errors"], 1)

    def test_a_run_with_no_targets_recorded_is_flagged_not_assumed(self):
        b = rf.bundle_from_run(
            [{"prompt": "read it", "tool_names": ["web_fetch"]}], [], "t", "Test3", "0.1.191")
        self.assertFalse(b["notes"]["targets_recorded"])


class ComparisonAgent(unittest.TestCase):
    """What the comparison agent did on affine.io in one turn. This is the shape of a 2 on every
    item, and it is the target the AFTER run is measured against."""

    def bundle(self):
        return rf.bundle(
            task="affine-read-website", agent="Misc", image="n/a",
            user_messages=["https://affine.io/ read this website"],
            assistant_messages=[
                "Epoch 661, about 21 minutes each. 21 of 199 green since e441, and 2 of the last 24. "
                "10 kings: newest UID 511 at z 1.14, strongest UID 517 at z 2.11. 8 valid queue "
                "batches. Registration costs about 1.01 TAO. Learning rate 5e-7, KL 0.02."
            ],
            tool_calls=[
                call("web_fetch", "https://affine.io/", output_empty=True),
                call("bash", "curl -s https://affine.io/"),
                call("bash", "curl -s https://affine.io/k1.json", error="curl: (35) SSL connect error"),
                call("bash", "curl -s https://affine.io/k1.json"),
                call("bash", "curl -s https://affine.io/k1/tables.json"),
            ],
            slots_filled=11, slots_total=11,
        )

    def test_one_turn_no_nudges_full_marks_on_every_exercised_item(self):
        s = rf.score(self.bundle())
        self.assertEqual(s["items"]["task_success"]["score"], 2)
        self.assertEqual(s["items"]["investigation"]["score"], 2)
        self.assertEqual(s["items"]["retry"]["score"], 2)
        self.assertEqual(s["items"]["autonomy"]["score"], 2)
        self.assertEqual(s["items"]["paging"]["score"], 2, s["items"]["paging"]["why"])
        self.assertEqual(s["score"], s["out_of"])

    def test_the_ssl_retry_is_what_earns_the_retry_item(self):
        b = self.bundle()
        b["tool_calls"] = [c for c in b["tool_calls"] if not (c["target"].endswith("k1.json") and not c["error"])]
        self.assertEqual(rf.item_retry(b)["score"], 0)


class Test3Shaped(unittest.TestCase):
    """Arbos on 0.1.191: three nudges, a thin answer, and a claim that contradicts the data it had.

    In evidence (internal/agent-has-bash-but-is-never-told.md): three nudges, and the tool set it
    called was web_fetch / web_search / history_search / memory_search / post_chart / routines_* /
    peek / codemode and nothing outside it. The bubble wording is not in evidence and is not
    invented here — the case carries only counts and the one quoted claim from the brief.
    """

    def test_no_shell_means_no_escalation_and_three_nudges_floor_autonomy(self):
        b = rf.bundle(
            task="affine-read-website", agent="Test3", image="0.1.191",
            user_messages=["https://affine.io/ read this website", "go on", "continue", "and the rest?"],
            assistant_messages=["Submissions are closed (accepting_submissions: false)."],
            tool_calls=[call("web_fetch", "https://affine.io/", output_empty=True)],
            slots_filled=2, slots_total=11,
        )
        s = rf.score(b)
        self.assertEqual(s["items"]["autonomy"]["score"], 0, s["items"]["autonomy"]["why"])
        self.assertEqual(s["items"]["investigation"]["score"], 0)
        self.assertEqual(s["items"]["task_success"]["score"], 0)
        self.assertIsNone(s["items"]["retry"]["score"], "no transient error occurred: n/a, not 0")

    def test_a_claimed_cause_with_no_failing_call_behind_it_scores_zero(self):
        b = rf.bundle(
            task="santa-teresa-listings", agent="Test3", image="0.1.191",
            user_messages=["<exact wording not recovered>"],
            assistant_messages=["The chart tool call failed, so there is no chart."],
            tool_calls=[call("web_search", "santa teresa listings")],
            produced_count=16, stated_count=15, asked_for_count=True,
        )
        s = rf.score(b)
        self.assertEqual(s["items"]["no_confabulation"]["score"], 0)
        self.assertEqual(s["items"]["counts"]["score"], 0)
        self.assertIn("off by 1", s["items"]["counts"]["why"])

    def test_a_fact_contradicted_by_the_agents_own_data_scores_zero(self):
        """On affine.io it asserted submissions were closed against a queue the same data reported
        as enabled. A contradiction outranks the agentd-log caveat: it needs no tool record."""
        b = rf.bundle(
            task="rs1-affine-read-website", agent="Test3", image="0.1.191",
            user_messages=["https://affine.io/ read this website"],
            assistant_messages=["Submissions are closed (`accepting_submissions: false`)."],
            tool_calls=[], notes={"agentd_log_only": True},
            contradictions=["'Submissions are closed' against queue.enabled true and 8 valid batches"],
        )
        v = rf.item_no_confabulation(b)
        self.assertEqual(v["score"], 0)
        self.assertIn("queue.enabled", v["evidence"][0])

    def test_the_same_claim_is_na_when_only_the_agentd_log_is_available(self):
        """AF-782: the reply_only guard returns before the tool_call T3 row is written, so an
        aborted call leaves no record. The agentd log cannot settle confabulation on 0.1.191."""
        b = rf.bundle(
            task="santa-teresa-listings", agent="Test3", image="0.1.191",
            user_messages=["<exact wording not recovered>"],
            assistant_messages=["The chart tool call failed, so there is no chart."],
            tool_calls=[], notes={"agentd_log_only": True},
        )
        self.assertIsNone(rf.item_no_confabulation(b)["score"])


class Test4Shaped(unittest.TestCase):
    """Arbos on 0.1.191: five fetches of one URL where `peek` was offered and never used."""

    def test_five_fetches_of_one_url_score_zero_on_paging(self):
        b = rf.bundle(
            task="llms-txt-read", agent="Test4", image="0.1.191",
            user_messages=["read the instruction sheet"],
            assistant_messages=["Here is what the sheet says."],
            tool_calls=[call("web_fetch", "https://relay.arbos.life/llms.txt") for _ in range(5)],
        )
        v = rf.item_paging(b)
        self.assertEqual(v["score"], 0)
        self.assertIn("5 times", v["why"])

    def test_a_second_fetch_is_forgiven_when_the_result_was_paged(self):
        b = rf.bundle(
            task="llms-txt-read", agent="x", image="0.1.191",
            user_messages=["read the sheet"], assistant_messages=["done"],
            tool_calls=[call("web_fetch", "https://relay.arbos.life/llms.txt"),
                        call("mcp__arbos__peek", "h1"),
                        call("web_fetch", "https://relay.arbos.life/llms.txt")],
        )
        self.assertEqual(rf.item_paging(b)["score"], 2)

    def test_a_cache_busting_query_does_not_hide_a_refetch(self):
        b = rf.bundle(
            task="llms-txt-read", agent="x", image="0.1.191",
            user_messages=["read the sheet"], assistant_messages=["done"],
            tool_calls=[call("web_fetch", "https://relay.arbos.life/llms.txt"),
                        call("web_fetch", "http://relay.arbos.life/llms.txt?cb=2"),
                        call("web_fetch", "https://relay.arbos.life/llms.txt/")],
        )
        self.assertEqual(rf.item_paging(b)["score"], 0)


class Dodging(unittest.TestCase):
    def test_a_generic_non_answer_scores_zero_and_a_pick_scores_two(self):
        shared = dict(task="best-agent", agent="Test3", image="0.1.191",
                      user_messages=["what is the best AI agent?"], tool_calls=[])
        dodge = rf.bundle(assistant_messages=["It depends on your use case; there is no single best."], **shared)
        pick = rf.bundle(assistant_messages=["Claude Code, for its tool use on long tasks."], **shared)
        self.assertEqual(rf.item_commits(dodge)["score"], 0)
        self.assertEqual(rf.item_commits(pick)["score"], 2)

    def test_a_hand_score_beats_the_phrase_match(self):
        b = rf.bundle(task="t", agent="a", image="i", user_messages=["q"], tool_calls=[],
                      assistant_messages=["It depends, but I would take Claude Code."], committed=True)
        self.assertEqual(rf.item_commits(b)["score"], 2)


class NaNotZero(unittest.TestCase):
    def test_an_unrecovered_transcript_scores_na_not_zero(self):
        """The Santa Teresa prompt wording was not recovered. A bundle with no checklist and no
        tool calls must leave those items out of the denominator, so nobody reads a missing
        transcript as a failing agent."""
        b = rf.bundle(task="santa-teresa-listings", agent="Test3", image="0.1.191",
                      user_messages=["<exact wording not recovered>"], assistant_messages=[], tool_calls=[])
        s = rf.score(b)
        self.assertIn("task_success", s["na"])
        self.assertIn("retry", s["na"])
        self.assertIn("paging", s["na"])
        self.assertIn("counts", s["na"])
        self.assertLess(s["out_of"], rf.MAX_SCORE)

    def test_an_unreadable_transcript_and_a_silent_turn_are_different_findings(self):
        """A bundle that could not read the bubbles scores the wording items `n/a`; a turn that
        genuinely said nothing scores `commits` 0. Both read as empty text otherwise."""
        unread = rf.bundle(task="t", agent="Test4", image="0.1.191", user_messages=["q"],
                           assistant_messages=[], tool_calls=[], notes={"text_in_evidence": False})
        silent = rf.bundle(task="t", agent="Test4", image="0.1.191", user_messages=["q"],
                           assistant_messages=[], tool_calls=[])
        self.assertIsNone(rf.item_commits(unread)["score"])
        self.assertIsNone(rf.item_no_confabulation(unread)["score"])
        self.assertEqual(rf.item_commits(silent)["score"], 0)
        self.assertIsNone(rf.item_no_confabulation(silent)["score"])

    def test_the_summary_keeps_na_out_of_the_share(self):
        full = rf.score(ComparisonAgent().bundle())
        thin = rf.score(rf.bundle(task="t", agent="a", image="i", user_messages=["q"],
                                  assistant_messages=["Claude Code."], tool_calls=[]))
        block = rf.summarize([full, thin])
        self.assertEqual(block["tasks"], 2)
        self.assertEqual(block["out_of"], full["out_of"] + thin["out_of"])
        self.assertTrue(rf.section(block)[0] == "")
        self.assertTrue(any("Resourcefulness" in l for l in rf.section(block)))


if __name__ == "__main__":
    unittest.main()
