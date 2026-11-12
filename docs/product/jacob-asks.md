# Jacob's asks, and what holds each one

Every row here is something Jacob asked for. Each one names a test that fails if the app stops
doing it. `scripts/ci/jacob-asks-check.py` enforces the pairing on every PR, so an ask cannot
quietly lose its guard and a new ask cannot land without one.

A visual ask with no test is the failure this file exists to stop: it regresses on some unrelated
rebase, nobody notices, and it surfaces weeks later in a screenshot.

## The rules the check enforces

1. **Every `[feedback <slug>]` tag in `apps/ios/Arbos/Arbos` has a row here.** Answer a new ask,
   tag it in the code, add the row.
2. **Every row names a guard that exists** — a suite (`BlockedStateTests`) or one case
   (`ChatSearchCloseTests.testATapOnAResultClosesThenReveals`) in `ArbosTests` / `ArbosUITests`.
3. **The guard names its ask**, in its own doc comment or its suite's, so the pairing is checked
   rather than asserted here.
4. **The guard is not switched off.** A `throw XCTSkip` that always runs means the ask is
   unguarded. A conditional `XCTSkipIf` escape (sources not beside the bundle) is allowed, since
   it still runs in CI; `scripts/ci/jacob-guard-gate.py` is what stops a new one being added to a
   guard in a PR.

Naming a suite guards an ask with all of its cases; naming a case is for a suite that covers
several asks. Prefer the narrower one.

Deleting a row, or the test it names, needs sign-off — see **Changing a guard** below.

## The register

| ask | asked | platform | what it must do | guard |
| --- | --- | --- | --- | --- |
| `agent-bubble-inset-2026-10-22` | 2026-10-22 | iPhone | An agent's bubble leaves 76 pt on the far side, the measured Grok number, not the old 71. | `JacobAskGuardTests.testTheAgentBubbleKeepsItsSeventySixPointInset` |
| `typing-indicator-left-2026-10-23` | 2026-10-23 | iPhone | The presence row over the composer names who is typing and which agents are working, one mark per handle, newest first. | `PresenceRowTests` |
| `group-library-links-2026-10-23` | 2026-10-23 | iPhone | A group's Library lists links the way the agent pane does, through the one shared list builder. | `JacobAskGuardTests.testAGroupsLibraryListsLinksThroughTheSharedBuilder` |
| `mention-chip-hover-2026-10-24` | 2026-10-24 | iPhone | A mention chip is accent on accent at 14 %, 30 % under the finger, so it visibly answers a touch. | `JacobAskGuardTests.testTheMentionChipStillAnswersTheFinger` |
| `agent-details-remove-sections-ios-2026-10-25` | 2026-10-25 | iPhone | The agent Details tab keeps its Coding and Routines cards hidden. | `JacobAskGuardTests.testTheAgentDetailsTabKeepsCodingAndRoutinesHidden` |
| `agent-pane-run-by-link-ios-2026-10-25` | 2026-10-25 | iPhone | Someone else's agent shows a "Run by …" line that opens its owner; my own agent shows none. | `BlockRowTests.testForeignAgentRunByLineIsAnOwnerLink` |
| `create-sheet-avatar-upload-generate-ios-2026-10-25` | 2026-10-25 | iPhone | The create sheet offers Upload and Generate only — the shape grid and pastel swatches are gone. | `AvatarPickerTabsTests.testTheCreateSheetOffersUploadAndGenerateOnly` |
| `image-caption-bubble-ios-2026-10-25` | 2026-10-25 | iPhone | A picture's caption wraps at the bubble's width, not the tall picture's. | `ImageCaptionBubbleTests` |
| `iphone-agent-pane-tabs-2026-10-25` | 2026-10-25 | iPhone | Each kind of pane keeps its own tab set, in its own order and wording. | `ProfilePaneStructureTests.testKindsKeepTheirOwnTabSets` |
| `iphone-bottom-search-bar-2026-10-25` | 2026-10-25 | iPhone | Home's bottom edge is one app-wide search bar; the old Telegram-style tab bar is gone. | `BottomSearchBarTests` |
| `iphone-composer-grok-parity-2026-10-25` | 2026-10-25 | iPhone | The composer matches Grok's measured geometry, every metric landing on a pixel. | `ComposerMetricsTests` |
| `ios-header-glass-2026-10-25` | 2026-10-25 | iPhone | The chat header is Liquid Glass — back, name pill, ⋯ — as in Grok's reference. | `HeaderGlassStructureTests` |
| `jump-to-latest-instant-ios-2026-10-25` | 2026-10-25 | iPhone | ↓ from deep history lands at once, re-cutting to one opening page instead of animating a 360-row rebuild. | `JumpToLatestWindowTests` |
| `library-chat-only-ios-2026-10-25` | 2026-10-25 | iPhone | Only things actually shared in a chat reach the Library; job rows and agent pages never do. | `LibraryListTests.testTypeLabelsAndMenus` |
| `library-list-ios-2026-10-25` | 2026-10-25 | iPhone | The Library is one list by day: five rows then Show more, compact times, the home list's day words. | `LibraryListTests` |
| `markdown-tables-ios-2026-10-25` | 2026-10-25 | iPhone | A GFM table draws as a real table with tappable link cells, not a monospace block of raw markup. | `MarkdownTableTests` |
| `profile-message-button-ios-2026-10-25` | 2026-10-25 | iPhone | A person's hero carries one Message action pill, under the subtitle. | `MessageButtonTests` |
| `blocked-state-ios-2026-10-26` | 2026-10-26 | iPhone | Someone I blocked has their rows hidden from me, the DM header tagged and the composer replaced by the blocked bar. | `BlockedStateTests` |
| `blocked-state-relay-2026-10-26` | 2026-10-26 | relay | A refused push explains itself in words, carrying the relay's own reason and never rewriting its code. | `JacobAskGuardTests.testARefusedPushExplainsItselfInWords` |
| `chart-bubble-ios-2026-10-26` | 2026-10-26 | iPhone | A chart draws as a chart, and the fallback picture that comes with it stays hidden. | `ChartCardTests` |
| `chat-density-ios-2026-10-26` | 2026-10-26 | iPhone | People chats use more of the screen on both sides than agent chats do. | `ChatDensityMetricsTests` |
| `code-highlighting-ios-2026-10-26` | 2026-10-26 | iPhone | A fenced block is coloured exactly as on the Mac, in Grok's six colours, with indent guides. | `CodeHighlightTests` |
| `error-messages-ios-2026-10-26` | 2026-10-26 | iPhone | Failures reach the screen as one written notice, never a raw error string or ad-hoc red text. | `ErrorUILintTests` |
| `iphone-add-members-2026-10-26` | 2026-10-26 | iPhone | Add members lists one Agents section: my agents first and pickable, then the others. | `AddMembersActionTests` |
| `iphone-chat-search-cmd-f-2026-10-26` | 2026-10-26 | iPhone | A hardware ⌘F opens the Search-in-chat bar and focuses it. | `ChatSearchShortcutTests` |
| `iphone-header-composer-sizing-2026-10-26` | 2026-10-26 | iPhone | One top bar for every screen, at the shared control inset. | `ControlMetricsTests` |
| `iphone-list-top-fade-2026-10-26` | 2026-10-26 | iPhone | Rows scrolling under the top bar fade out rather than cutting off against it. | `ListTopFadeTests` |
| `machine-page-ios-2026-10-26` | 2026-10-26 | iPhone | Settings → Machine shows the approved rows, gauges, warning card and per-agent breakdown. | `MachineMetricsTests` |
| `math-render-ios-2026-10-26` | 2026-10-26 | iPhone | Display math reaches the transcript as a math block in either delimiter, never as literal dollars. | `JacobAskGuardTests.testDisplayMathSurvivesBothDelimiters` |
| `profile-pane-toggle-ios-2026-10-26` | 2026-10-26 | iPhone | Tapping a person or agent in a chat opens their profile; tapping again closes it. Every route behaves the same. | `ProfileToggleTests` |
| `search-settings-ios-2026-10-26` | 2026-10-26 | iPhone | A Settings row opened from search lands on that page. | `SearchSettingsTests` |
| `secrets-agent-picker-ios-2026-10-26` | 2026-10-26 | iPhone | Every Secrets form picks agents through the one shared picker. | `AgentPickerTests` |
| `shared-search-matcher-ios-2026-10-26` | 2026-10-26 | iPhone | Entity search ranks exact over prefix over word-start, name before handle, diacritics folded — the Mac's rule. | `SharedSearchTests` |
| `status-row-gap-ios-2026-10-26` | 2026-10-26 | iPhone | The status row sits Grok's 42 px under the last bubble rather than stacking beneath it. | `ComposerMetricsTests.testStatusRowSettlesIntoTheTranscriptTail` |
| `add-your-agent-row-removal-ios-2026-10-27` | 2026-10-27 | iPhone | A pane carries at most one add row. | `OneAddRowPerPaneTests` |
| `add-your-agent-row-removal-mac-2026-10-27` | 2026-10-27 | Mac | The same one-add-row rule on the Mac's panes. | `OneAddRowPerPaneTests` |
| `chat-markdown-ios-2026-10-27` | 2026-10-27 | iPhone | Chat markdown parses the full block set, quotes nesting inside quotes. | `ChatMarkdownTests` |
| `composer-attachment-removal-ios-2026-10-27` | 2026-10-27 | iPhone | A pending attachment's ✕ is always drawn, at a full-size hit target, named for VoiceOver. | `ComposerMetricsTests.testEveryMetricLandsOnAPixel` |
| `confirm-alerts-ios-2026-10-27` | 2026-10-27 | iPhone | A consequential action asks first, through the one centred confirm — never a hand-built alert. | `ConfirmFormTests` |
| `copy-image-messages-ios-2026-10-27` | 2026-10-27 | iPhone | Copy on a picture message puts the picture on the pasteboard. | `MessagePasteboardTests` |
| `forward-attachments-ios-2026-10-27` | 2026-10-27 | iPhone | Forward and Copy read one accessor, so they take the same parts of a message. | `HiddenJobRowAttachmentTests` |
| `group-row-avatar-ios-2026-10-27` | 2026-10-27 | iPhone | A group shows its own picture through the resolver; the placeholder only when it has none. | `JacobAskGuardTests.testAGroupFallsToThePlaceholderOnlyWhenItHasNothing` |
| `group-settings-ios-2026-10-27` | 2026-10-27 | iPhone | A plain member sees no Edit group on the group pane. | `JacobAskGuardTests.testAPlainMemberHasNoEditGroupAndTheRigCanOnlyBeARealRole` |
| `group-settings-permissions-2026-10-27` | 2026-10-27 | iPhone | A permission refusal reads as the relay's own sentence. | `UserErrorTests.testAPermission403ReadsAsTheRelaysSentence` |
| `iphone-chat-search-cmd-f-toggle-2026-10-27` | 2026-10-27 | iPhone | ⌘F on an open bar closes it again — a toggle, like the Mac's. | `ChatSearchShortcutTests` |
| `link-embeds-ios-2026-10-27` | 2026-10-27 | iPhone | A message's first bare URL gets the relay's unfurled card, unless the message turns previews off. | `LinkPreviewCardTests` |
| `link-icons-ios-adopt-2026-10-27` | 2026-10-27 | iPhone | Search and the Library draw the one link tile at the row's 36 pt, rather than each inventing a glyph. | `JacobAskGuardTests.testSearchAndLibraryBothDrawTheOneLinkTile` |
| `link-icons-ios-tile-2026-10-27` | 2026-10-27 | iPhone | A link tile shows its site's icon, fetched once per host, with a placeholder for a site that has none. | `LinkTileTests` |
| `neon-dots-ios-2026-10-27` | 2026-10-27 | iPhone | No coloured status dot anywhere: activity is the braille spinner, unread is the count pill. | `StatusDotLintTests` |
| `search-toggle-click-outside-mac-2026-10-27` | 2026-10-27 | Mac | The Mac's search bar closes on a click away, by the same rule the iPhone uses. | `ChatSearchCloseTests.testTapOutsideClosesAndSwipeClosesOnlyWithoutAKeyword` |
| `toast-stack-ios-2026-10-27` | 2026-10-27 | iPhone | A repeated failure becomes "×N" on the standing toast instead of a second one. | `ToastStackTests` |
| `working-braille-spinner-ios-2026-10-27` | 2026-10-27 | iPhone | A row's status slot shows one thing: the spinner while the agent works, else the unread mark, else nothing. | `WorkingSpinnerTests` |
| `arbonites-blank-chat-ios-2026-10-28` | 2026-10-28 | iPhone | An empty read over a chat that has rows is a failed read, not an empty chat: what is on screen stays. | `BlankTranscriptGuardTests` |
| `devices-hide-revoked-ios-2026-10-28` | 2026-10-28 | iPhone | A revoked device drops off the devices list. | `DevicesVisibleTests` |
| `group-composer-placeholder-ios-2026-10-28` | 2026-10-28 | iPhone | A group composer's placeholder names the group. | `ComposerPlaceholderTests` |
| `grouped-messages-ios-2026-10-28` | 2026-10-28 | iPhone | A run of messages from one sender draws its avatar once, on the run's first message. | `GroupedMessagesTests` |
| `iphone-composer-glass-2026-10-28` | 2026-10-28 | iPhone | The composer's pills are Liquid Glass. | `PillGlassTests` |
| `iphone-composer-see-through-2026-10-28` | 2026-10-28 | iPhone | Rows scrolling under the chat bar stay readable through and between the glass pills — a fade, never an opaque band. | `BottomBarFadeTests` |
| `iphone-emoji-picker-reaction-2026-10-28` | 2026-10-28 | iPhone | A pick from the emoji keyboard sets the reaction once; the repeat reports that used to toggle it straight off are dropped. | `ReactionPickTests` |
| `iphone-filter-tab-taps-2026-10-28` | 2026-10-28 | iPhone | A filter tab is Apple's full 44 pt tall to the finger, whatever the capsule's height. | `HomeFilterTabsUITests` |
| `iphone-home-bar-fades-2026-10-28` | 2026-10-28 | iPhone | Home's search bar floats over the rows on the shared fade, with no background of its own. | `HomeBarFadeTests` |
| `iphone-search-close-2026-10-28` | 2026-10-28 | iPhone | The ✕, a tap on the messages area and a swipe on an empty query all close the search bar through one path. | `ChatSearchCloseTests` |
| `mention-chip-highlight-2026-10-28` | 2026-10-28 | iPhone | A mention chip is one pill with even insets and its picture on the centre line, wherever it falls. | `MentionChipSnapshotTests` |
| `outside-agents-wake-ios-2026-10-28` | 2026-10-28 | iPhone | The wake form tells clearing apart from leaving alone: an untouched field is not patched, a blank one is cleared. | `JacobAskGuardTests.testTheWakeFormTellsClearingApartFromLeavingAlone` |
| `paste-rich-text-ios-2026-10-28` | 2026-10-28 | iPhone | A words-only pasteboard stages text, not a phantom attachment. | `MessagePasteboardTests.testAWordsOnlyPasteboardStagesNothing` |
| `picture-reply-quote-ios-2026-10-28` | 2026-10-28 | iPhone | A reply carrying a picture keeps its quote, above the first media card, once. | `PictureReplyQuoteTests` |
| `right-pane-glass-cards-ios-2026-10-28` | 2026-10-28 | iPhone | Pane cards are a translucent wash with a hairline rim under Glass, the solid grey under Matte. | `PaneCardGlassTests` |
| `server-member-names-2026-10-28` | 2026-10-28 | iPhone | A channel bubble names its sender even when they share no group with me. | `ServerChannelSenderTests` |
| `sidebar-pinning-ios-2026-10-28` | 2026-10-28 | iPhone | Pins are one synced list, top first, cached so the section is there at launch. | `HomePinsTests` |
| `sidecar-sessions-ios-2026-10-28` | 2026-10-28 | iPhone | The Session log card shows the newest four lines worth reading; the plumbing stays behind View all. | `JacobAskGuardTests.testTheSessionLogCardShowsTheNewestFourRealLines` |
| `archived-chat-stays-archived-2026-10-29` | 2026-10-29 | iPhone | Opening a hidden chat only views it; it takes new activity or Unarchive to bring it back. | `HiddenChatOpenTests` |
| `scaled-timestamp-clip-mac-2026-10-29` | 2026-10-29 | iPhone + Mac | My message's hover/swipe time stays in the column on a scaled display or large Text Size — the peek column grows with text size on iPhone; the Mac reserves a fixed time + rail lane. | `ChatDensityMetricsTests.testSwipeTimeColumnGrowsWithTextSize` |
| `draft-chat-from-search-ios-2026-10-29` | 2026-10-29 | iPhone | A DM started from search lives on its own screen only: the home list never shows it, and leaving before a send drops it. | `DraftDMTests` |
| `full-picker-reaction-ios-2026-10-29` | 2026-10-29 | iPhone | A keycap or text-default symbol picked from the full picker reaches the row; a bare letter or digit does not. | `ReactionPickTests.testPickedKeycapReachesTheDocRow` |
| `image-viewer-close-button-2026-10-29` | 2026-10-29 | iPhone | The viewer's close control stays legible on a white, dark or busy picture. | `ViewerChromeTests` |
| `iphone-search-result-jump-2026-10-29` | 2026-10-29 | iPhone | A tap inside a search hit means "take me to this message" and nothing else — never a profile. | `ChatSearchCloseTests.testATapOnAResultClosesThenReveals` |
| `mention-list-overlay-ios-2026-10-29` | 2026-10-29 | iPhone | The mention list floats over the transcript; the bubbles below it do not slide up while it is open. | `MentionListOverlayUITests` |
| `reaction-chip-anchor-ios-2026-10-29` | 2026-10-29 | iPhone | A reaction chip stays anchored to its bubble. | `ReactionChipAnchorTests` |
| `composer-send-crossfade-2026-10-30` | 2026-10-30 | iPhone | The mic and the send arrow cross-fade: the mic while the field is empty or recording, the arrow the moment there are words. | `ComposerMetricsTests.testVoiceWaveGlyphIsGroksFiveBars` |
| `doubled-sender-prefix-2026-10-30` | 2026-10-30 | iPhone | A home row never prints the handle twice: the other person of a 1:1 is already named by the row's header. | `HomePreviewWhoTests` |
| `iphone-search-blank-ios27-2026-10-31` | 2026-10-31 | iPhone | A keyword with hits draws them — no blank result list on iOS 27. | `ChatSearchCloseUITests.testAKeywordWithHitsDrawsThem` |
| `group-chat-spacing-ios-2026-11-01` | 2026-11-01 | iPhone | A group transcript's avatar column is a fixed one, so rows do not shift as the composer moves. | `ChatDensityMetricsTests` |
| `iphone-bottom-fade-newest-bubble-2026-11-01` | 2026-11-01 | iPhone | The bottom fade is placed from the composer's controls, so a status row or error note does not drag it up over the rows. | `BottomBarFadeTests` |
| `iphone-blank-chat-on-open-2026-11-03` | 2026-11-03 | iPhone | A chat's first frame has its rows: no blank transcript on open. | `TranscriptFirstFrameUITests` |
| `time-dividers-2026-11-05` | 2026-11-05 | iPhone | A centred time line appears on a new local day or an hour's gap, by the core's shared rule. | `TimeDividerTests` |
| `typing-anchor-2026-11-08` | 2026-11-08 | iPhone | The presence row belongs to the end of the transcript: it scrolls away with the end it describes rather than riding the bar over old bubbles. | `PresenceAnchorUITests` |
| `attachment-in-composer-2026-11-09` | 2026-11-09 | iPhone | A picked image shows as a thumbnail inside the composer box, Grok-style, before it is sent. | `ComposerParityUITests.testAStagedPicturePreviewsInsideTheComposer` |
| `composer-hold-highlight-2026-11-09` | 2026-11-09 | iPhone | Resting a finger on the composer lights the box, as Grok's does, while the system menu comes up. | `ComposerParityUITests.testTheBoxVisiblyLightsUnderAHoldingFinger` |
| `agent-access-link-ios-2026-11-10` | 2026-11-10 | iPhone | Settings → Agent access link creates, copies/shares and revokes the same agent links as the Mac, so a phone can hand an agent a key. | `AgentAccessLinkUITests` |
| `connected-apps-hidden-until-implemented-2026-11-10` | 2026-11-10 | iPhone | Settings lists no Connected apps row until the feature ships (Mac already omits it; `connected_apps_stays_off_the_settings_list_until_implemented` holds that side). | `ConnectedAppsHiddenUITests` |
| `solo-1to1-no-agent-reply-quote-ios-2026-11-10` | 2026-11-10 | iPhone | In a solo 1:1 agent chat, an agent bubble never shows a reply quote header — even when the row still carries replyTo from an older box. A group or channel still quotes. A human's own reply still quotes. | `SoloAgentReplyQuoteTests` |
| `solo-1to1-no-agent-reply-quote-mac-2026-11-10` | 2026-11-10 | Mac | Same rule on Mac: solo 1:1 agent bubbles never quote; groups and channels still do; a human's own reply still does. Guarded in-tree by `crates/arbos-mac` `solo_agent_reply_quote_tests`; the iPhone suite is the named twin. | `SoloAgentReplyQuoteTests` |
| `iphone-composer-less-opaque-2026-11-11` | 2026-11-11 | iPhone | The Glass composer field pill fills at 12 %, not the 22 % that still read as a dark plate over the transcript. | `ComposerParityTests.testTheFieldPillLetsTheTranscriptThrough` |
| `connection-pill` | — | iPhone | The header pill shows the worst of the disagreeing signals, and says nothing at all while the link is live. | `JacobAskGuardTests.testTheConnectionPillShowsTheWorstSignalAndStaysQuietWhenLive` |
| `in-call-bar` | — | iPhone | The in-call strip's chip stays 36 pt, a recorded exception to the one app-wide control height. | `JacobAskGuardTests.testTheInCallChipKeepsItsRecordedException` |
| `reply-preview-fixed-height` | — | iPhone | The reply bar above the field is one row high whatever it quotes: the quote is flattened to a line and capped. | `JacobAskGuardTests.testTheReplyPreviewIsOneRowWhateverItQuotes` |
| `2005` | — | iPhone | After an unblock, the rooms standing refused are asked to resend; a room that is not refused ignores the nudge. | `SessionRoomTests.testChatToRetryNamesOnlyMembershipAndSettingsChanges` |
| `closed-app-call-ring-2026-11-11` | 2026-11-11 | iPhone | VoIP and APNs tokens re-upload on every foreground pass so a closed iPhone can still ring after the relay drops a dead Apple token (TestFlight AFjyB3HHRZ7GzKtZHyvxIUw / #2995). | `JacobAskGuardTests.testForcedTokenUploadRunsEvenWhenTheHexIsUnchanged` |

## Superseded

Kept so the tag in the code resolves, and so nobody re-adds the behaviour by mistake.

| ask | asked | why | what replaced it |
| --- | --- | --- | --- |
| `iphone-composer-bottom-fade-2026-10-28` | 2026-10-28 | Jacob, on TestFlight 202610281715985037: "we should actually see the text behind the search bar". The opaque band this asked for was the wrong answer. | `iphone-composer-see-through-2026-10-28`, guarded by `BottomBarFadeTests` |

## Changing a guard

Deleting a row, deleting the test it names, or skipping that test unconditionally is a decision
about whether Jacob still gets what he asked for. `scripts/ci/jacob-guard-gate.py` fails a PR
that does any of the three unless its description carries a line naming the ask:

```
Guard-change: <ask-id> — <why, and who agreed>
```

Write the reason for a person reading the ask's history a year from now, not for the gate.
