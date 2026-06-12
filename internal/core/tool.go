package core

import "encoding/json"

// ToolCall is a model-requested invocation. Args is raw JSON so the kernel
// never needs to know a tool's argument shape; the ToolRuntime owns decoding.
type ToolCall struct {
	ID   string
	Name string
	Args json.RawMessage
}

// ArgsOrEmpty returns Args, or the empty JSON object when Args is unset. Provider
// adapters that serialize a tool call to a wire format that requires an object
// use this instead of hand-rolling the same nil check.
func (t ToolCall) ArgsOrEmpty() json.RawMessage {
	if len(t.Args) == 0 {
		return json.RawMessage("{}")
	}
	return t.Args
}

// ContentType discriminates a ContentBlock. The set is closed and small on
// purpose: text, inline image, and inline file (a document like a PDF the
// provider parses natively) are the shapes a coding tool result (and, from the
// multimodal phase, a message) needs.
type ContentType string

const (
	BlockText  ContentType = "text"
	BlockImage ContentType = "image"
	BlockFile  ContentType = "file"
)

// ImageData is an inline, base64-encoded image.
type ImageData struct {
	Data     string `json:"data"`
	MimeType string `json:"mimeType"`
}

// FileData is an inline, base64-encoded document (e.g. a PDF). Providers that
// accept native document input parse it themselves (text extraction and, for
// scanned pages, OCR); the kernel never decodes it. Name labels the file on the
// wire where the provider surfaces it (OpenAI's file input wants a filename).
type FileData struct {
	Data     string `json:"data"`
	MimeType string `json:"mimeType"`
	Name     string `json:"name,omitempty"`
}

// ContentBlock is one piece of multimodal content. Exactly one payload field is
// set, selected by Type (Text for BlockText, Image for BlockImage, File for
// BlockFile). It is the shared shape for non-text tool output today and
// multimodal message content, mirroring the text|image|document content union
// real providers expose. Tagged for stable JSON round-trip through the event
// log and the provider wire.
type ContentBlock struct {
	Type  ContentType `json:"type"`
	Text  string      `json:"text,omitempty"`
	Image *ImageData  `json:"image,omitempty"`
	File  *FileData   `json:"file,omitempty"`
}

// ToolResult is the outcome of dispatching a ToolCall.
//
// Content is the canonical text the model sees and remains the primary channel;
// a text-only tool sets only Content, exactly as before. Blocks carries non-text
// output (images) and Details carries structured data the model never sees (a
// diff, a unified patch, a truncation record, a spill-file path) for renderers
// and compaction. Both are additive: a result without them serializes and
// projects identically to the original string-only shape, so the persisted log
// needs no version bump or upcast.
type ToolResult struct {
	CallID  string
	Content string
	Blocks  []ContentBlock  `json:",omitempty"`
	Details json.RawMessage `json:",omitempty"`
	IsError bool
	// Terminate hints that the agent loop should stop after the current tool
	// batch. The engine stops early only when every result in the batch sets it
	// (pi's contract), so a single terminating tool in a mixed batch does not end
	// the turn. Additive; a tool that does not set it behaves as before.
	Terminate bool `json:",omitempty"`
}

// ToolSchema is the provider-facing description of a callable tool. Parameters
// is a JSON Schema document. In the kernel proper these are generated from Go
// structs at build time (see ADR on tool-schema codegen) so the schema can
// never drift from the handler.
type ToolSchema struct {
	Name        string
	Description string
	Parameters  json.RawMessage

	// ReadOnly marks a tool that observes without mutating state (a web fetch, a
	// file read, a search). The engine may dispatch a batch of exclusively
	// read-only calls concurrently; any write forces the whole batch sequential.
	// This is the only classification the kernel needs — finer same-resource
	// conflict analysis lives in the tool layer, which understands the args.
	ReadOnly bool
}
