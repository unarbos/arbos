package openai

import "encoding/json"

// Wire types: the JSON shapes of the OpenAI Chat Completions API. They are
// unexported and live only in this adapter — the kernel never sees them. Only
// the fields the kernel needs are modeled.

type wireRequest struct {
	Model           string             `json:"model"`
	Messages        []wireMessage      `json:"messages"`
	Tools           []wireTool         `json:"tools,omitempty"`
	Stream          bool               `json:"stream"`
	StreamOptions   *wireStreamOptions `json:"stream_options,omitempty"`
	Temperature     *float64           `json:"temperature,omitempty"`
	MaxTokens       int                `json:"max_tokens,omitempty"`
	ReasoningEffort string             `json:"reasoning_effort,omitempty"`
}

type wireStreamOptions struct {
	IncludeUsage bool `json:"include_usage"`
}

// wireMessage.Content is `any` because Chat Completions accepts either a plain
// string (the common text case) or an array of typed content parts (multimodal:
// text plus image_url). A text-only message marshals to a string exactly as
// before; a message with image Parts marshals to the parts array.
type wireMessage struct {
	Role       string         `json:"role"`
	Content    any            `json:"content"`
	ToolCallID string         `json:"tool_call_id,omitempty"`
	ToolCalls  []wireToolCall `json:"tool_calls,omitempty"`
}

// wireContentPart is one element of a multimodal content array.
type wireContentPart struct {
	Type     string        `json:"type"`
	Text     string        `json:"text,omitempty"`
	ImageURL *wireImageURL `json:"image_url,omitempty"`
	File     *wireFile     `json:"file,omitempty"`
}

type wireImageURL struct {
	URL string `json:"url"` // a data: URL for inline base64 images
}

// wireFile is the Chat Completions inline file input: a base64 data: URL plus
// the original filename (the API surfaces it to the model). Used for documents
// the model parses natively, e.g. PDFs.
type wireFile struct {
	Filename string `json:"filename,omitempty"`
	FileData string `json:"file_data"` // a data: URL for inline base64 files
}

type wireToolCall struct {
	ID       string         `json:"id"`
	Type     string         `json:"type"`
	Function wireToolCallFn `json:"function"`
}

type wireToolCallFn struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// wireTool is one entry of the Chat Completions `tools` array. A user-defined
// tool is a function: Type "function" with Function set. A provider-side server
// tool (OpenRouter's `openrouter:web_search` etc.) carries its type directly and
// omits Function, with optional tuning under Parameters — so Function is a
// pointer that drops out of the JSON for server tools.
type wireTool struct {
	Type       string          `json:"type"`
	Function   *wireToolFn     `json:"function,omitempty"`
	Parameters json.RawMessage `json:"parameters,omitempty"`
}

type wireToolFn struct {
	Name        string          `json:"name"`
	Description string          `json:"description,omitempty"`
	Parameters  json.RawMessage `json:"parameters,omitempty"`
}

// Streaming response shapes.

type wireChunk struct {
	Choices []wireChoice `json:"choices"`
	Usage   *wireUsage   `json:"usage"`
}

type wireChoice struct {
	Delta        wireDelta `json:"delta"`
	FinishReason string    `json:"finish_reason"`
}

type wireDelta struct {
	Content     string              `json:"content"`
	Reasoning   string              `json:"reasoning"`
	ToolCalls   []wireToolCallDelta `json:"tool_calls"`
	Annotations []wireAnnotation    `json:"annotations"`
	Images      []wireDeltaImage    `json:"images"`
}

// wireDeltaImage is one element of the message/delta `images` array: a
// generated image OpenRouter attaches to the assistant message (the
// image_generation server tool / image output modality). The URL is a
// base64 data: URL.
type wireDeltaImage struct {
	Type     string       `json:"type"`
	ImageURL wireImageURL `json:"image_url"`
}

// wireAnnotation is one element of the message `annotations` array. OpenRouter
// standardizes web-search sources (across native, Exa, etc.) onto OpenAI's
// url_citation shape; in a stream they arrive on the delta of the final
// content chunk. Only the url_citation variant is modeled — other annotation
// types are ignored.
type wireAnnotation struct {
	Type        string `json:"type"`
	URLCitation struct {
		URL        string `json:"url"`
		Title      string `json:"title"`
		Content    string `json:"content"`
		StartIndex int    `json:"start_index"`
		EndIndex   int    `json:"end_index"`
	} `json:"url_citation"`
}

type wireToolCallDelta struct {
	Index    *int   `json:"index"`
	ID       string `json:"id"`
	Function struct {
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
	} `json:"function"`
}

type wireUsage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}
