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
	IntentInterrupt        IntentKind = "interrupt"
	IntentResume           IntentKind = "resume"
	IntentApprovalResponse IntentKind = "approval_response"
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

// PromptIntent starts a turn with user-supplied text.
type PromptIntent struct {
	Text string `json:"text"`
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
func (InterruptIntent) Kind() IntentKind        { return IntentInterrupt }
func (ResumeIntent) Kind() IntentKind           { return IntentResume }
func (ApprovalResponseIntent) Kind() IntentKind { return IntentApprovalResponse }
func (SetModelIntent) Kind() IntentKind         { return IntentSetModel }
func (CompactIntent) Kind() IntentKind          { return IntentCompact }
