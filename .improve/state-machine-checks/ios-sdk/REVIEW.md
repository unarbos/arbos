# Finite media boundary source review

Scoped PASS for the exact seven product hashes in SOURCE. The final 9d9d6c212 change removes four
isCurrent default values from the public and private helper contracts; all other
bytes in those two files are identical to the reviewed media/link checkpoint.
All existing producers retain explicit predicates, and missing future arguments
now fail compilation. The first six-file
checkpoint and the two-file LinkPreview successor were read before each SDK build;
Transcript is shared by those layers. Supplied stale/unavailable origins retain
their original receipt and cannot fall back to a replacement account. Contexts use
existing lifetimes/state; no per-row API factory or extra ownership epoch is added.

All three actual LinkPreviewCard constructors receive the row's captured origin.
Both direct cards and the BlockView/TypedBody paths, recursive quotes and typed
children carry it unchanged, preventing a stale row from capturing B during later
body rendering. Both private icon/image fetches admit at MediaFetch's MainActor
header/URL boundary and recheck after each await. The finite actual-app inventory
finds 10 MediaFetch.bytes calls, 2 copyAttachment producers and 1 copyMessage producer;
all explicitly supply isCurrent. This is helper caller closure, not an all-API or
whole-app runtime proof.

Nested viewer/library copy, image/video/file loading, share/forward and Photos
result paths retain the same origin. An already admitted Photos submission may
finish externally; its changes block reads captured item/data rather than B's
ambient authority, and post-result UI remains fenced. This is not external rollback.
Anonymous public pictures preserve Close/read/zoom and empty authorization headers;
private actions remain unavailable. No concrete additional defect was found in the
requested finite boundary. Existing album order, URL rules, layouts, gesture timing,
formats, codecs and physical calibration stay unchanged.

The author's selected-source 80-case and mutation checks are separate from this
reviewer's real SDK compile/typecheck evidence. Actual XCTest assertions, rendered UI,
Photos effects, physical devices, account/provider services and final canonical CI
were not run here. Earlier checkpoints remain private and immutable; this handoff
contains no binaries, modules or raw logs.
