# Bounded H3 metadata ownership review: PASS

Read-only comparison of actual agent.rs/settings.rs against frozen rpc-owner; exact two source hashes are MAP-REVIEW-SOURCE.json and remained unchanged during review. No test/build/SDK/provider/service execution by reviewer. Author native response-barrier and package gates are separate evidence in GATES.json.

CurrentPi captures both opaque BootTicket and exact Arc<RpcChild> while the slot is Ready. Metadata commits revalidate Ready+ticket+pointer under the same slot mutex as replacement/shutdown; no ambient replacement handle is re-captured after the old request awaits. Conditional refusal recovery begins a boot only if that exact owner still matches, then returns its newly committed CurrentPi. If a later boot wins before this return, subsequent checks still reject the returned stale ticket.

New multi-lock sites follow model→chat→slot or a suffix of that order. commit_pi_model uses model→slot; conditional respawn uses chat→slot; boot publication and session metadata use model→chat→slot. transition_pi only updates the existing slot and projections, without acquiring metadata locks in reverse. Child close/spawn awaits occur after those locks release. The switch-session store write remains synchronous within the commit fence; its existing database failure policy is unchanged.

Thinking-level follow-up checks the captured owner and sends to that same Arc, including a legitimately recovered replacement owned by this request. A replacement occurring after the preflight can receive no stale effect through an ambient getter: the effect remains aimed at the captured old child. Success/refusal/error branches use the same owner for model commits; failed restore cannot overwrite a newer child's metadata. Ready publication preserves the existing MainPiBoot cancellation/cleanup guard.

The actual native test seam uses real RPC response delivery then delays continuation while replacing the child. Positive controls include current session/model/fallback updates and same-request refusal recovery. This is stronger than directly invoking a reducer or fake completion callback.

Scope: this read validates child-lifetime ownership, not a global transaction among independently changing desired configuration, external child state and SQLite. It does not establish ordering for unrelated same-child concurrent configuration revisions, provider behavior, remote cleanup or production performance. No API/timeout/calibration change is inferred from this scoped read.
