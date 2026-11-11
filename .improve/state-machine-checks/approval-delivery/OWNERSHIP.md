# H4 correction

The core ApprovalState/ApprovalOutcome and pending/approved/consumed Store CAS were already explicit. The genuine High seam was cross-owner delivery admission, not the mere presence of enum fields. Pending delivery is now a single tagged type owning its sender, transitioned at actual decision/park edges; durable decision/claim is one transaction and only committed admission can publish Approved to active callers. Deferred remains an intentional durable unclaimed grant, claimed later by exact tool/canonical input. Committed inline channel loss is a spent abandoned attempt, never restored. SQL failure retains pending state; terminal decision absorption stays in actual SQL.

Five original semantic REDs,12new native tests,64finite×16event transition traces, existing6grace/park controls, and all40 approval tests on identical space-free source cover the shared boundary. No missing native capacity or broad model rewrite is asserted.

Reviewer preservation correction: non-approved inline send loss keeps the prior deferred notification path; only committed Approved authority remains spent/Abandoned. Actual SQL-edge receiver-close regression was1P1F before this narrow amendment.
