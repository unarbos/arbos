package core

// KernelEventKind is the serialization discriminator for a KernelEvent and the
// sealed-interface marker. In-process frontends (the Go TUI) switch on the
// concrete type; out-of-process frontends (web, desktop, remote arbos, the
// Phase-7 control seam) switch on Kind() across a wire without reflection.
// Mirrors EventPayload.Kind() for the persisted log. See ADR-0018.
type KernelEventKind string

const (
	KernelEventMessageDelta    KernelEventKind = "message_delta"
	KernelEventReasoningDelta  KernelEventKind = "reasoning_delta"
	KernelEventToolStarted     KernelEventKind = "tool_started"
	KernelEventToolFinished    KernelEventKind = "tool_finished"
	KernelEventTurnComplete    KernelEventKind = "turn_complete"
	KernelEventInterrupted     KernelEventKind = "interrupted"
	KernelEventError           KernelEventKind = "error"
	KernelEventQueued          KernelEventKind = "queued"
	KernelEventApprovalRequest KernelEventKind = "approval_request"
	KernelEventClarifyRequest  KernelEventKind = "clarify_request"
)

// KernelEvent is something a session emits as it works. Frontends render these;
// they do not poll kernel state. Kind() seals the set and is the serialization
// discriminator.
type KernelEvent interface {
	Kind() KernelEventKind
}

// MessageDelta is an incremental chunk of assistant text.
type MessageDelta struct {
	Text string `json:"text"`
}

// ReasoningDelta is an incremental chunk of assistant reasoning/thinking.
type ReasoningDelta struct {
	Text string `json:"text"`
}

// ToolStarted announces a tool invocation about to run.
type ToolStarted struct {
	Call ToolCall `json:"call"`
}

// ToolFinished reports a tool invocation's result.
type ToolFinished struct {
	Result ToolResult `json:"result"`
}

// StopReason explains why a turn ended, so a frontend can render a normal
// completion differently from one that hit a guardrail. A turn ending because
// it ran out of iterations is NOT an error — it is a legible terminal state
// with whatever partial work it produced. See ADR-0020.
type StopReason string

const (
	StopAnswered   StopReason = "answered"   // model produced a final answer with no further tool calls
	StopMaxSteps   StopReason = "max_steps"  // hit the iteration cap before answering
	StopTerminated StopReason = "terminated" // a tool batch signalled early termination (every result set Terminate)
)

// TurnComplete signals the turn ended normally. StopReason distinguishes a real
// answer from hitting the step limit; Usage carries the turn's aggregate token
// accounting so a frontend can show cost without querying the log.
type TurnComplete struct {
	FinalResponse string     `json:"final_response"`
	StopReason    StopReason `json:"stop_reason"`
	Usage         Usage      `json:"usage"`
}

// Interrupted signals the turn was cancelled before completing.
type Interrupted struct{}

// ErrorKind categorizes a turn error so frontends render it appropriately and
// callers can decide whether a retry is worth offering. See ADR-0020.
type ErrorKind string

const (
	ErrProvider ErrorKind = "provider" // upstream model/transport failure
	ErrHistory  ErrorKind = "history"  // could not load the session log
	ErrPersist  ErrorKind = "persist"  // could not append to the log (source-of-truth integrity)
	ErrInternal ErrorKind = "internal" // panic, unhandled intent, or kernel invariant breach
)

// ErrorEvent reports an error that ended the turn. Category lets a frontend
// distinguish "rate-limited, try again" from "fatal"; Retryable is the kernel's
// hint about whether re-issuing the same prompt could succeed. (Field is named
// Category, not Kind, to avoid colliding with the Kind() discriminator method.)
type ErrorEvent struct {
	Category  ErrorKind `json:"category"`
	Retryable bool      `json:"retryable"`
	Err       string    `json:"error"`
}

// Queued acknowledges a prompt that arrived while a turn was in flight; it will
// run after the current turn finishes (FIFO). This replaces the skeleton's
// silent drop of intents-while-busy — the user always learns their input was
// kept. See ADR-0020.
type Queued struct {
	Text string `json:"text"`
}

// ApprovalRequest pauses the turn to ask whether a tool call may proceed. The
// turn blocks until a matching ApprovalResponseIntent (same RequestID) arrives
// — the suspend-and-await control-flow rule (ADR-0018). Emitted, e.g., before a
// destructive tool call when the approval policy requires confirmation.
type ApprovalRequest struct {
	RequestID RequestID `json:"request_id"`
	Call      ToolCall  `json:"call"`
	Reason    string    `json:"reason,omitempty"`
}

// ClarifyRequest pauses the turn to ask the user a free-text question, answered
// by a matching ClarifyResponseIntent.
type ClarifyRequest struct {
	RequestID RequestID `json:"request_id"`
	Question  string    `json:"question"`
}

func (MessageDelta) Kind() KernelEventKind    { return KernelEventMessageDelta }
func (ReasoningDelta) Kind() KernelEventKind  { return KernelEventReasoningDelta }
func (ToolStarted) Kind() KernelEventKind     { return KernelEventToolStarted }
func (ToolFinished) Kind() KernelEventKind    { return KernelEventToolFinished }
func (TurnComplete) Kind() KernelEventKind    { return KernelEventTurnComplete }
func (Interrupted) Kind() KernelEventKind     { return KernelEventInterrupted }
func (ErrorEvent) Kind() KernelEventKind      { return KernelEventError }
func (Queued) Kind() KernelEventKind          { return KernelEventQueued }
func (ApprovalRequest) Kind() KernelEventKind { return KernelEventApprovalRequest }
func (ClarifyRequest) Kind() KernelEventKind  { return KernelEventClarifyRequest }
