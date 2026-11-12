# H11 boundary inventory

Scope: billed cumulative voice/vision usage settlement, HIGH because the old cursor and monetary debit could commit independently. Actual states are now transaction-private observed usage/charge decisions and a committed `VoiceSettlement`; SQLite rollback disposes every uncommitted preparation. `PostfactDebit` distinguishes paid, clamped and policy-refused facts. There is no persistent preparing flag to recover or duplicate unused state model.

Storage remains the existing voice_calls and credit_ledger/users rows. Illegal combinations reproduced: ledger committed/cursor stale; cursor advanced/ledger failure; two debit receipts for one cumulative observation. The shared Db transaction is used by the actual HTTP charge_report edge and the generic postfact account helper.

Regression prerequisites now retained: both aborting and ignored-row faults, separate-Db concurrent report, replay and reordered totals, voice/vision ordering, total/part/inner thresholds, unmetered behavior, legacy default 0/default100 initialization, cap and balance-clamp distinctions, actual input decoder checks,32 finite seeded actual-helper laws. Informational machine time is separate remaining scope.

Root review added two actual SQL-ignore cases for skipped legacy user creation and
required account-cap writes. Those failed against the initial H11 candidate
(0 pass/2 fail); the required cap UPDATE now verifies exactly one affected row,
rolling back initialization, usage and debit when metadata did not persist.
Valid configured 0.5 daily soft-cap behavior and successful retry remain pinned.
