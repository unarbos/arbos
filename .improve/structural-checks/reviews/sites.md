# Independent sites route parity review

PASS for the scoped routing/fixture change. Core policy source is byte-identical.

The Worker now applies the same subdomain and app restrictions at both routing
schemes: reserved prefix union, xn-- prefix refusal, lowercase ASCII shape and
bounds, and no double dash in app names. Inner double dashes remain valid in a
non-punycode prefix, as the core allows. about/terms match the authoritative
reserved list; valid allocated names are not newly excluded by these changes.

The shared fixture has35 route vectors and102 distinct reserved names. I compared
the actual Worker reserved set with that fixture: exact equality, not only a
subset. Rust separately asserts fixture equality with its authoritative constants.
Both test adapters independently derive outputs from real production routing;
the fixture is not a second reimplementation of either router.

Worker user-landings, trailing-slash redirects and invalid-app responses remain
separate from site resolution, and its path-scheme passthrough is an intentional
adapter difference. The core fixture constructs hosts for subdomain/two-level
only; it does not claim Worker path-scheme hosting. Host normalization is explicit
in the Node test adapter, consistent with fetch's URL hostname handling.

The bounded cases cover both new validator uses, lengths, case, Unicode/rest
bytes, literal percent paths, nested/slash behavior and positive inner-dash/name
boundaries. Inspection found no changed fetch/R2/cache/write behavior and no
remaining actionable defect in this diff. The tests are pure route checks, not
live URL/Cloudflare/deployment integration or proof of every possible input.
The source owner retains Node baseline9 failed/28 passed and after37/37 evidence; the actual Rust fixture
execution must also pass before publication (not claimed by this read-only review).
