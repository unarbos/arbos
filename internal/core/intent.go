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
	IntentCompact          IntentKind = "compact"
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
type PromptIntent struct {
	Text  string         `json:"text"`
	Parts []ContentBlock `json:"parts,omitempty"`
}

// SteerIntent replaces the in-flight turn with new user text. When idle it
// behaves like PromptIntent; when a turn is active it cancels silently (no
// Interrupted event) and discards any prompts queued during that turn. Parts
// mirrors PromptIntent: optional attached non-text content.
type SteerIntent struct {
	Text  string         `json:"text"`
	Parts []ContentBlock `json:"parts,omitempty"`
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

// SetModelIntent switches the model used for subsequent turns of a session. It
// applies between turns (when the session is idle); the engine owns the
// per-session override so no shared state is mutated mid-turn. Mirrors pi's RPC
// set_model.
type SetModelIntent struct {
	Model string `json:"model"`
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
func (CompactIntent) Kind() IntentKind          { return IntentCompact }
