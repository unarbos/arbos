# P6 hosted Swift terminal evidence

Owned-fork run42828800182, attempt1, head84cb882feefc327e67167b723c9441015ab65944 completed with a real test failure. Job129873404857 built FFI and build-for-testing successfully, then failed step11 fastlane test. This is not capacity cancellation or zero-test evidence.

| Immutable P6 gate | Passed | Failed | Skipped | Unique case records |
|---|---:|---:|---:|---:|
| Unit text records |550|1|0|551|
| UI cases |54|1|13|68|

Unit counts use complete unique lowercase per-case records; none needed interleaving repair. The text supplies no unit-target aggregate or xcresult bundle. UI counts agree with the68-case target summary,13 skips and one failure event. The two text logs are the only artifact entries; no expected-failure count is inferred beyond the supplied per-case statuses.

The unit failure is AddMembersActionTests.testAnOldRelayWithoutTheBatchRouteGetsThePerItemCalls (35.859s). Neither artifact nor job console includes its failed assertion detail. The complete fixture and the direct AppModel addMembers/installRelayForTests plus five RelayAPI request/member/agent methods match BASE byte-for-byte. Other P6 AppModel/RelayAPI behavior changed, so this comparison does not establish its cause. It remains a historical unresolved failure, not an automatic flaky/setup classification or a claimed P6 fix.

The UI failure is ChatSearchResultJumpUITests.testAResultTapOnTheSenderAvatarAlsoStaysInTheChat, line204: after the sender-avatar tap on long-412, the expected message is not realized and the transcript holds g6–g9. Its test, ChatView and Transcript sources match BASE/P8, but the model/API differs. The prior shared search-reveal method passed in this run; the different failing method must not be grouped under that earlier failure merely by topic. The root cause is unresolved here.

Later integrated logs independently record the fallback unit passing0.045s in the corrected full predecessor and0.036s in the final lifetime follow-up, and this exact UI method passing20.839s in the predecessor. LATER-INTEGRATED.json records those existing observations and log hashes. Different later source coverage does not make this immutable hosted run pass or prove its historical cause. No old-run rerun or speculative source/test fix was performed.

GitHub artifact13777124404 was117,634bytes; ZIP SHA256029567f1b94430209ac8f847b2ebf6de5e5d5f7938b94df1d1f2542e3b836b3d matches official metadata. Before extraction, exactly two regular0644 UTF8 text entries were checked, with no absolute/parent/backslash paths, links, encryption or NUL bytes; archive<1MiB, each entry<25MiB,total<50MiB. File/digest details are in ZIP-INSPECTION.json. Raw ZIP/logs and runner paths stay private. The actual job console proves ARBOS_RELAY_URL=http://127.0.0.1:1 written and verified for both test targets; that is not a whole-simulator no-egress claim.

To reproduce the text analysis from a repository containing the pinned Git objects, re-obtain only the declared artifact through the read-only GitHub Actions API, verify its digest and inspect the above entry/sizing/type constraints before extracting the two logs under raw/. Fetch the declared job console as job-private.log and verify JOB.json's digest. Run python3 -I -B PATH_TO_PACKET/analyze.py from the repository root. The stdlib parser checks raw digests, exact nonzero counts, source identities and the distinct failing cases; it executes no downloaded content. No capability URL or credentials are retained here.

This read-only collection did not rerun, cancel or mutate CI, launch apps/tests, change source/index/refs or contact a product service. Earlier task actions are documented separately in REPORT. Existing hosted-v3 and earlier packets were not changed. CI wall times are verification evidence, not application performance claims.
