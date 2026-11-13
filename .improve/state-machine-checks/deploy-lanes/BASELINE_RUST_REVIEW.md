# AUX-H7 bounded independent source review

Read-only review of the six named script files, no shell execution, privileged command, configuration/env read, network/provider call or production operation. Author source/tests were still in progress; this is not a frozen final PASS. No shellcheck executable is present in this reviewer's PATH/tools evidence; the prior verified tool there is actionlint1.7.12 only.

## Actionable finding

**High: unexpected canary execution/cancellation statuses count as passed.** Current extracted `run_canary_smokes` records primary failure only for rc1; every other primary exit falls through. The host variant explicitly allows rc2 as inconclusive, but calls every non1/non2 value "ok". Thus a missing interpreter/command (126/127), cancellation (130/143), or arbitrary75 can lead to return0 and the later default-snapshot switch without a successful smoke. This is inherited gate code moved into a newly shared edge, not a claim that the lock helper introduced it. Pin actual wrapper fake-smoke127 and143 before fixing. Preserve documented0 success/2 inconclusive semantics; unexpected outcomes should stop operationally before default/real roll, and must not accidentally be treated as the controller's own Deferred75. Whether unknown infrastructure failure sets the manual image hold is a different policy from rc1 and must not be invented implicitly.

## Coherent scoped mechanisms

- Queue hook and worker snapshot/ack use one stable .queue.lock. Snapshot contains a strictly validated bounded five-field record; ack compares exact bytes under that lock, so acknowledging an old snapshot does not remove a replacement request.
- Lane admission is checked and held by FD7 for the full callback, with EXIT cleanup and signal status. Nonblocking contention returns reserved75; the900s canary wait keeps the request without smoke/default switch/hold. A live descriptor prevents stale marker age from authorizing overlap with a new participating owner.
- The lane subprocess returns callback status; relay request ack remains conditional on success. Worker canary admission75 is distinguished before later snapshot mutation. Voice remains separate from the relay/canary/www socket-disruption lane, as before.
- Both installers supply the new helper alongside the scripts that source it. This review did not execute installers/selftests or read their actual host configuration.

## Limits

Legacy marker compatibility is conservative, but these locks are local-process coordination, not a distributed transaction or protection against a root operator bypassing the scripts. Kernel flock/FD cancellation behavior and actual adapters are the author's mock-backed gate responsibility; this review makes no deployment acceptance claim. Refresh reload failure still follows its existing best-effort policy; no new claim of transactional Caddy publication is made.

Current source hashes recorded for handoff (re-read if author changes after review):

```json
{
  "deploy-state.sh": "6c68bd9eaae7f2207c2de9c9e3c61d9e64ab0417efd177c32b820aa5b56294be",
  "arbos-deploy-worker": "a7dcae0c62eebfa5404544ab77dcbb270d5eca8499458e72a41c77c1d8d40327",
  "arbos-deploy-hook": "4f1164b7c135bb339b637378d1f156b85ca7681cbf2690d720ae2b722732c7fd",
  "refresh-mac-download.sh": "afb1abb6a9d163960c4ae012b5dcddc2c69ec3eeae111882ff5d0ed2305ab58b",
  "install-deploy-hook.sh": "72ed346c312679b672c9d083045b358b4e96780283a1ba571e3009480292e8ac",
  "relay-server.sh": "e66ac627ff03389f4e90c1a1616a93a403ab3597aa45f96861f1b5b51832f94c"
}
```

## Reviewed correction

Author retained actual original-source failure on immutable worker blob d0fc83f9: one test group with four primary/host×127/143 subcases, all returned0 and failed semantically. The corrected actual run_canary_smokes now admits only0 or documented2, preserves rc1's existing image hold, and maps every other external result to an operational failure before default switch with no invented image hold. Source re-read confirms this correction; author reports all four after controls pass. No new tests were run by this reviewer. This resolves the actionable finding; the bounded source review is **PASS for this reviewed scope**, subject to the author's final adapter/law/hash freeze. No broad deployment acceptance claim follows.

A subsequent narrow source read identified one reserved-status collision still needing closure: an external smoke75 is returned raw, then wait_for_relay_lane interprets it as its own admission Busy/Deferred and retries for900s. Default remains blocked, but the operational failure is retried as contention. Author notified to distinguish external75 (for example normalize it to76) and pin primary/host75 before final freeze. The earlier scoped PASS for127/143 is therefore conditional on this last reviewed correction.

Final reserved-status correction re-read: the actual primary/host smoke edge now normalizes external75 to76, while126/127/130/143 retain their operational failures and0/1/2 retain prior policy. Author retained the pre-normalization repeat/Deferred RED and reports six unexpected-status subcases now execute once each and stop before hold/default. No other actionable defect found in the scoped queue/lane/installer read. **Scoped source review PASS**; author final gate/law acceptance still owns runtime/kernel verification. No tests rerun here.

Final reviewed source hashes:
```json
{
  "deploy-state.sh": "6c68bd9eaae7f2207c2de9c9e3c61d9e64ab0417efd177c32b820aa5b56294be",
  "arbos-deploy-worker": "dde2a80634e022df68b535dab93bcab4bcae2d3e2f435b1a18c6885ec55dcf82",
  "arbos-deploy-hook": "4f1164b7c135bb339b637378d1f156b85ca7681cbf2690d720ae2b722732c7fd",
  "refresh-mac-download.sh": "6df9ed3cee876ef6285495bd0e5295124939098ba2783a405479b69cfcd3815b",
  "install-deploy-hook.sh": "72ed346c312679b672c9d083045b358b4e96780283a1ba571e3009480292e8ac",
  "relay-server.sh": "e66ac627ff03389f4e90c1a1616a93a403ab3597aa45f96861f1b5b51832f94c"
}
```

Final ack-caller delta reviewed: roll_image now calls deploy_request_ack_commit(agentd, resolved_sha). Its shared helper validates the actual bounded queue record and compares its current SHA under the same .queue.lock before transition-gated removal. A replacement request for B is retained. Same-SHA acknowledgment is intentional satisfaction of the deployed minimum; the ordinary hook does not rewrite an already-equal request. The remaining delete of QUEUE/agentd.rolling is a distinct obsolete rolling marker, not the agentd request. No new finding in this delta. Final worker/helper hashes:
{
  "deploy-state.sh": "b61a22696ff68d0f47fdd9926fa1e392041f32d1e53043f344514545403a299a",
  "arbos-deploy-worker": "202b0e9515bf3655dece1e5325c6724f6fe66f2699816ffb66bded47883bfd84"
}
