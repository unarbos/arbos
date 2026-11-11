# Observe composer lifetime by identity

The completed integrated native run exposed an unreliable global live-count
assertion:0 live drafts versus an earlier baseline1 does not identify an owned
leak. The same counter can also hide a retained draft when another draft frees.
DEBUG instrumentation now keeps weak instance identities. The existing hosted
ChatView test retains pre-existing objects, observes only new weak references,
asserts that an open host owns a draft and that closing frees every observed draft.
Its public wrapper, window, autorelease pool and0.3-second settling remain.

Two deterministic XCTest controls prove that an unrelated release cannot produce
a false failure or hide an intentionally retained draft. Weak unrelated probes
are asserted alive before the explicit release and nil afterward. The observer
itself does not retain the draft. All registry access is MainActor-isolated.
The complete ChatView outside the DEBUG observer block and the Release class
projection are byte-identical to their respective preimages.

The existing ComposerMetricsTests.swift contains these runnable XCTest cases;
the project's fenced native test lane executes them. A selected actual-class
Foundation/Combine check also passes11 assertions; it uses an inert mention
collaborator and does not stand in for UIKit execution. The integrated successor
accepts all559 unit cases, including one intentional expected-failure control,
and passes the complete unchanged five-case avatar/back-swipe UI class. Exact
case counts, source pins and the separate predecessor full-UI result are recorded
in the finish report. No final-source unfiltered full-UI pass is inferred.

The observation window captures all new draft identities rather than a direct
host-to-draft reference. An unrelated new draft could cause a false failure; it
cannot hide a retained captured draft. No global count or longer delay is used.
