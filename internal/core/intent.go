package core

// IntentKind is the serialization discriminator for an Intent and doubles as the
// sealed-interface marker: the set of intents is closed, and every variant
// declares its kind. This is what lets a frontend across a wire (web, desktop,
// remote arbos, the Phase-7 control seam) tag and route intents without
// reflection — mirroring how EventPayload uses Kind() for the persisted log.
// See ADR-0018.
type IntentKind string

const (
	IntentPrompt           IntentKind = "prompt"
	IntentSteer            IntentKind = "steer"
	IntentInterrupt        IntentKind = "interrupt"
	IntentResume           IntentKind = "resume"
	IntentApprovalResponse IntentKind = "approval_response"
	IntentQuestionResponse IntentKind = "question_response"
	IntentSetModel         IntentKind = "set_model"
	IntentSetWebSearch     IntentKind = "set_web_search"
	IntentSetWebFetch      IntentKind = "set_web_fetch"
	IntentSetImageGen      IntentKind = "set_image_gen"
	IntentCompact          IntentKind = "compact"
	IntentChatNote         IntentKind = "chat_note"
)

// Intent is something an outside actor (CLI, TUI, gateway, socket) asks a
// session to do. The kernel accepts only Intents — never direct method calls
// into the turn loop — which is what lets every frontend attach to the same seam
// without re-implementing turn logic. Kind() seals the set and is the
// serialization discriminator.
type Intent interface {
	Kind() IntentKind
}

// PromptIntent starts a turn with user-supplied text. Parts carries optional
// non-text content (images) attached to the prompt; it lands on the user
// Message's Parts so a vision-capable model sees it (ADR-0022). Text-only
// prompts leave Parts nil and behave exactly as before.
//
// Origin names the door the prompt arrived through when that door is NOT the
// session's bound frontend (e.g. "telegram" for a bridge injecting into an
// open web tab's actor). A non-empty Origin makes the actor echo the prompt
// as a Queued event even when idle, so the attached frontend renders text it
// did not send itself. The frontend's own prompts leave it empty — they are
// already rendered locally, and an echo would duplicate them.
type PromptIntent struct {
	Text   string         `json:"text"`
	Parts  []ContentBlock `json:"parts,omitempty"`
	Origin string         `json:"origin,omitempty"`
	// Author is the self-asserted display name of the human who sent this
	// prompt, set server-side from a shared-session guest's signed cookie (never
	// trusted from the client frame). It lands on the user Message's Author so
	// other doors and the model can tell participants apart in a multi-party
	// chat. Empty for the local operator and single-party sessions.
	Author string `json:"author,omitempty"`
}

// SteerIntent replaces the in-flight turn with new user text. When idle it
// behaves like PromptIntent; when a turn is active it cancels silently (no
// Interrupted event) and discards any prompts queued during that turn. Parts
// mirrors PromptIntent: optional attached non-text content.
type SteerIntent struct {
	Text  string         `json:"text"`
	Parts []ContentBlock `json:"parts,omitempty"`
	// Author mirrors PromptIntent.Author: the self-asserted display name of the
	// human who sent this steer, stamped server-side.
	Author string `json:"author,omitempty"`
}

// ChatNoteIntent is a human-to-human side-chat line: collaborators sharing a
// session talking to EACH OTHER, NOT prompting the agent. The actor appends it
// to the log and broadcasts it to every door but NEVER starts a turn, and it is
// excluded from the model projection — so it reaches people, not the model.
// Origin and Author mirror PromptIntent so the existing cross-door echo
// suppression and server-side name stamping (filterShareFrame) apply verbatim;
// for a share guest Author is overwritten server-side and never trusted from
// the client frame.
type ChatNoteIntent struct {
	Text   string         `json:"text"`
	Parts  []ContentBlock `json:"parts,omitempty"`
	Origin string         `json:"origin,omitempty"`
	Author string         `json:"author,omitempty"`
}

// InterruptIntent cancels the in-flight turn, if any.
type InterruptIntent struct{}

// ResumeIntent rehydrates a session from its persisted event log.
type ResumeIntent struct {
	SessionID SessionID `json:"session_id"`
}

// ApprovalResponseIntent answers an ApprovalRequest, unblocking a turn that
// paused to ask whether a tool call may proceed (suspend-and-await; ADR-0018).
// RequestID correlates the answer to the originating request.
type ApprovalResponseIntent struct {
	RequestID RequestID `json:"request_id"`
	Approved  bool      `json:"approved"`
	Reason    string    `json:"reason,omitempty"`
}

// QuestionAnswer is the user's answer to one Question: the selected option ids,
// or free text when they typed their own answer instead. Both empty = the
// question went unanswered.
type QuestionAnswer struct {
	QuestionID  string   `json:"question_id"`
	SelectedIDs []string `json:"selected_ids,omitempty"`
	OtherText   string   `json:"other_text,omitempty"`
}

// QuestionResponseIntent answers a QuestionRequest, unblocking a turn that
// paused to collect structured answers (suspend-and-await; ADR-0018).
// RequestID correlates the answer to the originating request. Details carries
// optional free text the user added alongside their selections; Skipped means
// they dismissed the form without answering.
type QuestionResponseIntent struct {
	RequestID RequestID        `json:"request_id"`
	Answers   []QuestionAnswer `json:"answers,omitempty"`
	Details   string           `json:"details,omitempty"`
	Skipped   bool             `json:"skipped,omitempty"`
}

// SetModelIntent switches the model used for subsequent turns of a session.
// Sent while idle it applies immediately; sent mid-turn (the set_model tool,
// or a frontend that didn't wait) it is deferred to the end of the running
// turn — the engine owns the per-session override and only ever mutates it at
// a turn boundary, so no shared state is touched mid-turn. Mirrors pi's RPC
// set_model.
type SetModelIntent struct {
	Model string `json:"model"`
}

// SetWebSearchIntent turns provider-side web search on or off for subsequent
// turns of a session. Like SetModelIntent it applies between turns (when idle)
// and the engine owns the per-session flag, so no shared state is mutated
// mid-turn. The flag is durable: it is persisted on the Session so a resumed
// session keeps it. See ADR-0027.
type SetWebSearchIntent struct {
	Enabled bool `json:"enabled"`
}

// SetWebFetchIntent turns provider-side web fetch on or off for subsequent
// turns of a session. Same idle-only, durable contract as SetWebSearchIntent,
// and independent of it. See ADR-0027.
type SetWebFetchIntent struct {
	Enabled bool `json:"enabled"`
}

// SetImageGenIntent turns provider-side image generation on or off for
// subsequent turns of a session. Same idle-only, durable contract as
// SetWebSearchIntent, and independent of it.
type SetImageGenIntent struct {
	Enabled bool `json:"enabled"`
}

// CompactIntent asks the session to compact its context now (pi's RPC compact),
// rather than waiting for the automatic preflight trigger. Applies when idle.
type CompactIntent struct{}

func (PromptIntent) Kind() IntentKind           { return IntentPrompt }
func (SteerIntent) Kind() IntentKind            { return IntentSteer }
func (InterruptIntent) Kind() IntentKind        { return IntentInterrupt }
func (ResumeIntent) Kind() IntentKind           { return IntentResume }
func (ApprovalResponseIntent) Kind() IntentKind { return IntentApprovalResponse }
func (QuestionResponseIntent) Kind() IntentKind { return IntentQuestionResponse }
func (SetModelIntent) Kind() IntentKind         { return IntentSetModel }
func (SetWebSearchIntent) Kind() IntentKind     { return IntentSetWebSearch }
func (SetWebFetchIntent) Kind() IntentKind      { return IntentSetWebFetch }
func (SetImageGenIntent) Kind() IntentKind      { return IntentSetImageGen }
func (CompactIntent) Kind() IntentKind          { return IntentCompact }
func (ChatNoteIntent) Kind() IntentKind         { return IntentChatNote }
