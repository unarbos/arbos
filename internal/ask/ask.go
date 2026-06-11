// Package ask is the agent's structured question to the user: a tool that
// pauses the turn, puts multiple-choice questions on screen (Cursor's
// AskQuestion), and returns the user's selections as the tool result. The
// suspend-and-await ride is the engine's (engine.Asker, the approval
// mechanism); this package owns only the tool's schema and the translation
// between its arguments and the answer the model reads.
//
// Wired on the top-level engine only (like delegate): a question needs a user
// watching, which a delegated child or scheduler wake does not have.
package ask

import (
	"context"
	"encoding/json"
	"fmt"
	"reflect"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/engine"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// Option is one selectable answer to a question.
type Option struct {
	ID    string `json:"id" desc:"Unique identifier for this option"`
	Label string `json:"label" desc:"Display text for this option"`
}

// Question is one multiple-choice question to present.
type Question struct {
	ID            string   `json:"id" desc:"Unique identifier for this question"`
	Prompt        string   `json:"prompt" desc:"The question text to display to the user, without the options."`
	Options       []Option `json:"options" desc:"Array of answer options (minimum 2 required)"`
	AllowMultiple bool     `json:"allow_multiple,omitempty" desc:"If true, user can select multiple options. Defaults to false."`
}

// Args are the arguments to the ask tool — Cursor's AskQuestion shape.
type Args struct {
	Questions []Question `json:"questions" desc:"Array of questions to present to the user (minimum 1 required)"`
	Title     string     `json:"title,omitempty" desc:"Optional title for the questions form"`
}

const description = "Collect structured multiple-choice answers from the user. Use this tool only when you are blocked on a decision that is genuinely the user's to make: one you cannot resolve from the request, the code, or sensible defaults. Each question should have at least 2 options; the user can always pick \"Other\" and answer in their own words, and may skip the form entirely. Use allow_multiple: true to allow multiple answers to be selected for a question. If you recommend a specific option, make it the first option in the list and add \" (Recommended)\" at the end of its label. When presenting the user with a set of discrete options or next steps, prefer this tool over listing them in your response text. The turn pauses until the user answers, so never call it from unattended work."

// RegisterTool adds the ask tool to a registry. Reflected at registration like
// delegate and notify (the documented ADR-0004 carve-out for dynamically wired
// tools).
func RegisterTool(reg *tool.Registry) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(Args{}))
	if err != nil {
		return fmt.Errorf("ask schema: %w", err)
	}
	spec := tool.NewRichSpec("ask", description, false,
		func(ctx context.Context, a Args) (tool.Result, error) {
			if err := validate(a); err != nil {
				return tool.Result{}, err
			}
			asker := engine.Asker(ctx)
			if asker == nil {
				return tool.Result{}, fmt.Errorf("ask: no interactive frontend is attached to answer questions")
			}
			resp, ok := asker(a.Title, toCore(a.Questions))
			if !ok {
				return tool.Result{}, fmt.Errorf("ask: interrupted while waiting for the user's answer")
			}
			// Details carries the structured response for renderers (the
			// transcript card, replay); the model reads the prose Content.
			details, _ := json.Marshal(struct {
				Answers []core.QuestionAnswer `json:"answers,omitempty"`
				Details string                `json:"details,omitempty"`
				Skipped bool                  `json:"skipped,omitempty"`
			}{resp.Answers, resp.Details, resp.Skipped})
			return tool.Result{Content: formatResponse(a, resp), Details: details}, nil
		})
	return reg.Register(spec, schema)
}

func validate(a Args) error {
	if len(a.Questions) == 0 {
		return fmt.Errorf("ask: at least one question is required")
	}
	for i, q := range a.Questions {
		if strings.TrimSpace(q.Prompt) == "" {
			return fmt.Errorf("ask: question %d has an empty prompt", i+1)
		}
		if len(q.Options) < 2 {
			return fmt.Errorf("ask: question %q needs at least 2 options", q.Prompt)
		}
	}
	return nil
}

func toCore(qs []Question) []core.Question {
	out := make([]core.Question, len(qs))
	for i, q := range qs {
		opts := make([]core.QuestionOption, len(q.Options))
		for j, o := range q.Options {
			opts[j] = core.QuestionOption{ID: o.ID, Label: o.Label}
		}
		out[i] = core.Question{ID: q.ID, Prompt: q.Prompt, Options: opts, AllowMultiple: q.AllowMultiple}
	}
	return out
}

// formatResponse renders the user's answers as the prose the model reads:
// each question with the chosen option labels (or the user's own words), the
// optional free-text addendum, or the skip.
func formatResponse(a Args, resp core.QuestionResponseIntent) string {
	byQuestion := make(map[string]core.QuestionAnswer, len(resp.Answers))
	for _, ans := range resp.Answers {
		byQuestion[ans.QuestionID] = ans
	}

	var b strings.Builder
	answered := false
	for _, q := range a.Questions {
		ans := byQuestion[q.ID]
		text := answerText(q, ans)
		if text == "" {
			text = "(no answer)"
		} else {
			answered = true
		}
		fmt.Fprintf(&b, "Q: %s\nA: %s\n\n", q.Prompt, text)
	}

	if resp.Skipped || !answered {
		s := "The user skipped the questions."
		if resp.Details != "" {
			s += "\n\nThey added: " + resp.Details
		}
		return s
	}
	out := "The user answered:\n\n" + strings.TrimSpace(b.String())
	if resp.Details != "" {
		out += "\n\nAdditional details: " + resp.Details
	}
	return out
}

func answerText(q Question, ans core.QuestionAnswer) string {
	labels := make([]string, 0, len(ans.SelectedIDs))
	for _, id := range ans.SelectedIDs {
		label := id
		for _, o := range q.Options {
			if o.ID == id {
				label = o.Label
				break
			}
		}
		labels = append(labels, label)
	}
	parts := make([]string, 0, 2)
	if len(labels) > 0 {
		parts = append(parts, strings.Join(labels, ", "))
	}
	if strings.TrimSpace(ans.OtherText) != "" {
		parts = append(parts, strings.TrimSpace(ans.OtherText))
	}
	return strings.Join(parts, " — ")
}
