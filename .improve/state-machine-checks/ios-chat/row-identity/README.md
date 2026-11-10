# Controlled row identity regression

The existing sender-avatar swipe, tap and vertical-drag cases failed because the transcript had no rendered rows in the canonical C19 run. Both header controls passed. Removing only the inner RowView `.id(account)` restored all five cases on the same owned iOS 26.2 simulator. The host account identity, account-aware row equality, unique message scroll anchors, gestures and tests stay unchanged.

The before evidence is five completed cases from an incomplete full run. The after gate selects only this existing class and is not a full-suite pass or a performance measurement. The additional repository goldens repair a separate private-input omission; the source-derived fixture proof shows these avatar cases use generated Swift rows and no external document/replay input.

All 2,689 expected source files, all 14 repository UI test sources, file modes, generated FFI and the loopback-relay test plan were verified. The parent process uses the reviewed exact testmanagerd socket exception to its network-denial policy. This is not universal simulator egress isolation. A full corrected, unfiltered unit/UI run remains required.
