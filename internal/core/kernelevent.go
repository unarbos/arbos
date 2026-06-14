package core

import "encoding/json"

// KernelEventKind is the serialization discriminator for a KernelEvent and the
// sealed-interface marker. In-process frontends (the Go TUI) switch on the
// concrete type; out-of-process frontends (web, desktop, remote arbos, the
// Phase-7 control seam) switch on Kind() across a wire without reflection.
// Mirrors EventPayload.Kind() for the persisted log. See ADR-0018.
type KernelEventKind string

const (
	KernelEventMessageDelta    KernelEventKind = "message_delta"
	KernelEventReasoningDelta  KernelEventKind = "reasoning_delta"
	KernelEventCitations       KernelEventKind = "citations"
	KernelEventImages          KernelEventKind = "images"
	KernelEventToolProgress    KernelEventKind = "tool_progress"
	KernelEventToolDetails     KernelEventKind = "tool_details"
	KernelEventToolStarted     KernelEventKind = "tool_started"
	KernelEventToolFinished    KernelEventKind = "tool_finished"
	KernelEventTurnComplete    KernelEventKind = "turn_complete"
	KernelEventInterrupted     KernelEventKind = "interrupted"
	KernelEventError           KernelEventKind = "error"
	KernelEventQueued          KernelEventKind = "queued"
	KernelEventApprovalRequest KernelEventKind = "approval_request"
	KernelEventQuestionRequest KernelEventKind = "question_request"
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

// Citations reports the web-search sources the provider grounded the current
// assistant message on. Unlike content, citations arrive only once the message
// is complete (they ride on the provider's final chunk), so they cannot be
// streamed as deltas — this event delivers them in one shot for the frontend to
// attach to the assistant turn it has been building. Presentation only; the
// sources are persisted on the assistant Message, not as this event. See
// ADR-0027.
type Citations struct {
	Citations []Citation `json:"citations"`
}

// Images delivers provider-generated images for the current assistant message
// (OpenRouter's image_generation server tool). Like Citations they ride the
// provider's final chunk, so they arrive in one shot after the content deltas
// and before turn_complete. Presentation only; the images are persisted on the
// assistant Message's Parts, not as this event.
type Images struct {
	Images []ContentBlock `json:"images"`
}

// ToolProgress reports a tool call's arguments still streaming in: the call's
// identity and how many argument bytes have arrived so far. It is emitted
// repeatedly while a large call is composed (e.g. writing a canvas), so a
// frontend can show live progress in the gap between the prose and the finished
// call — the same gap the streaming deltas leave silent. Presentation only;
// never persisted, like MessageDelta and ReasoningDelta.
type ToolProgress struct {
	CallID string `json:"call_id"`
	Name   string `json:"name"`
	Bytes  int    `json:"bytes"`
	// ArgsDelta is the argument-JSON fragment that just arrived (not the
	// cumulative buffer). A frontend accumulates these to render the call's
	// content streaming in — chiefly a write/edit's file body appearing live.
	ArgsDelta string `json:"args_delta,omitempty"`
}

// ToolDetails carries a running tool's presentation-only Details to frontends
// before its result lands, keyed by call id. It exists so a frontend can act
// on a side-channel fact the tool learns mid-execution — chiefly the job id a
// bash command is journaling to, which lets the terminal card offer "open as a
// terminal tab" (a live tail of the journal) while the command is still
// running, not only after it returns. Details mirrors ToolResult.Details (the
// same channel surfaces and sub-agents ride); the model never sees it.
// Presentation only; never persisted, like ToolProgress.
type ToolDetails struct {
	CallID  string          `json:"call_id"`
	Details json.RawMessage `json:"details"`
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
	StopAnswered    StopReason = "answered"     // model produced a final answer with no further tool calls
	StopMaxSteps    StopReason = "max_steps"    // hit the iteration cap before answering
	StopTerminated  StopReason = "terminated"   // a tool batch signalled early termination (every result set Terminate)
	StopLengthLimit StopReason = "length_limit" // provider stopped because the output token cap was hit
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
//
// Origin carries the PromptIntent's door marker when the prompt arrived from
// somewhere other than the bound frontend (e.g. "telegram"): the actor also
// echoes such prompts when idle, and a frontend renders them as the user
// speaking through that door rather than as a queue acknowledgment.
type Queued struct {
	Text   string `json:"text"`
	Origin string `json:"origin,omitempty"`
	// Parts carries the prompt's non-text content (images) so an echoed
	// cross-door prompt renders complete — a photo sent from a phone must
	// appear in the open web tab, not just its caption.
	Parts []ContentBlock `json:"parts,omitempty"`
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

// QuestionOption is one selectable answer to a Question.
type QuestionOption struct {
	ID    string `json:"id"`
	Label string `json:"label"`
}

// Question is one multiple-choice question put to the user.
type Question struct {
	ID            string           `json:"id"`
	Prompt        string           `json:"prompt"`
	Options       []QuestionOption `json:"options"`
	AllowMultiple bool             `json:"allow_multiple,omitempty"`
}

// QuestionRequest pauses the turn to collect structured answers from the user
// (the ask tool). The turn blocks until a matching QuestionResponseIntent
// (same RequestID) arrives — the same suspend-and-await control flow as
// ApprovalRequest (ADR-0018).
type QuestionRequest struct {
	RequestID RequestID  `json:"request_id"`
	Title     string     `json:"title,omitempty"`
	Questions []Question `json:"questions"`
}

func (MessageDelta) Kind() KernelEventKind    { return KernelEventMessageDelta }
func (ReasoningDelta) Kind() KernelEventKind  { return KernelEventReasoningDelta }
func (Citations) Kind() KernelEventKind       { return KernelEventCitations }
func (Images) Kind() KernelEventKind          { return KernelEventImages }
func (ToolProgress) Kind() KernelEventKind    { return KernelEventToolProgress }
func (ToolDetails) Kind() KernelEventKind     { return KernelEventToolDetails }
func (ToolStarted) Kind() KernelEventKind     { return KernelEventToolStarted }
func (ToolFinished) Kind() KernelEventKind    { return KernelEventToolFinished }
func (TurnComplete) Kind() KernelEventKind    { return KernelEventTurnComplete }
func (Interrupted) Kind() KernelEventKind     { return KernelEventInterrupted }
func (ErrorEvent) Kind() KernelEventKind      { return KernelEventError }
func (Queued) Kind() KernelEventKind          { return KernelEventQueued }
func (ApprovalRequest) Kind() KernelEventKind { return KernelEventApprovalRequest }
func (QuestionRequest) Kind() KernelEventKind { return KernelEventQuestionRequest }
