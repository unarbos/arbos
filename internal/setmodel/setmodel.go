// Package setmodel is the agent's hand on its own model selector: a tool that
// switches the current session's model and can persist the choice as the
// host's default for future sessions. The switch rides the same
// core.SetModelIntent the web composer's picker sends — the tool is just
// another door posting an intent — so the engine's turn-boundary discipline
// holds: the running turn finishes on its model and the next turn runs on the
// new one.
//
// The companion list_models tool surfaces the provider's live catalog (the
// same modelcatalog listing the web picker shows), so the agent can answer
// "what models are available?" and offer real ids instead of guessing — and
// set_model checks requested ids against that catalog, turning a typo or a
// nickname ("fable") into close-match suggestions instead of a blind switch
// that fails a turn later with a provider error.
//
// Wired on the top-level engine only (like delegate and ask): the deliver
// seam reaches sessions live on the host engine, which a delegated child's
// ephemeral engine is not.
package setmodel

import (
	"context"
	"fmt"
	"reflect"
	"sort"
	"strings"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/modelcatalog"
	"github.com/unarbos/arbos/internal/obs"
	"github.com/unarbos/arbos/internal/tool"
	"github.com/unarbos/arbos/internal/tool/jsonschema"
)

// Catalog fetches the provider's live model listing. Nil when the provider
// has no catalog endpoint (no LLM configured), which disables list_models and
// set_model's id validation.
type Catalog func(ctx context.Context) ([]modelcatalog.Model, error)

// Args are the arguments to the set_model tool.
type Args struct {
	Model   string `json:"model" desc:"Model id to switch to, exactly as the configured provider names it (e.g. an OpenRouter slug like anthropic/claude-opus-4.8). Checked against the provider catalog when one is available: an unlisted id is rejected with close matches instead of switching — use list_models to find the exact id."`
	Default bool   `json:"default,omitempty" desc:"Also save as the host's default model, so future sessions (and restarts without an explicit ARBOS_MODEL) start on it."`
	Force   bool   `json:"force,omitempty" desc:"Switch even when the id is not in the provider catalog (the catalog may lag brand-new models). Only after confirming the id is exactly right; a wrong id fails the next turn with a provider error."`
}

const description = "Switch the model this conversation runs on. The change applies from the next turn — the current turn finishes on the present model — and persists for this session across restarts. Set default:true to also make it the host-wide default for new sessions. Use when the user asks to change the model; when they name a model loosely (a nickname, a family name), find the exact id with list_models first rather than guessing."

// ListArgs are the arguments to the list_models tool.
type ListArgs struct {
	Query string `json:"query,omitempty" desc:"Case-insensitive substring filter on model id and display name (e.g. \"sonnet\", \"openai/\"). Empty lists the whole catalog, truncated."`
}

const listDescription = "List the models available from the configured provider — the same live catalog the web UI's model picker shows. Each line is an exact id set_model accepts, with the display name and context window. Use when the user asks what models are available, or to resolve a loose model name to its exact id before calling set_model or offering choices via ask."

// listMax caps a single listing so an unfiltered OpenRouter catalog (hundreds
// of models) doesn't flood the context; the truncation note tells the model
// to narrow with query.
const listMax = 60

// suggestMax caps the close-match list a rejected set_model returns.
const suggestMax = 8

// RegisterTool adds the set_model tool — and, when a catalog is available,
// the list_models tool — to a registry. deliver posts an intent to a live
// session on the host engine (engine.Deliver, late-bound by the host since
// tools are composed before the engine exists); setDefault persists the host
// default-model preference, nil when the host has no settings store; catalog
// is the provider's live model listing, nil when it has no catalog endpoint.
func RegisterTool(reg *tool.Registry, deliver func(core.SessionID, core.Intent) bool, setDefault func(string) error, catalog Catalog) error {
	schema, err := jsonschema.Reflect(reflect.TypeOf(Args{}))
	if err != nil {
		return fmt.Errorf("set_model schema: %w", err)
	}
	spec := tool.NewSpec("set_model", description, false,
		func(ctx context.Context, a Args) (string, error) {
			model := strings.TrimSpace(a.Model)
			if model == "" {
				return "", fmt.Errorf("set_model: model is required")
			}
			cor, _ := obs.From(ctx)
			if cor.SessionID == "" {
				return "", fmt.Errorf("set_model: no session in context")
			}
			// Validate against the live catalog when one is reachable. A
			// failed or empty fetch falls through to the blind switch — the
			// catalog is a courtesy, not a gate on a provider hiccup.
			if catalog != nil && !a.Force {
				if models, err := catalog(ctx); err == nil && len(models) > 0 {
					if reply := rejectUnlisted(models, model); reply != "" {
						return reply, nil
					}
				}
			}
			if !deliver(core.SessionID(cor.SessionID), core.SetModelIntent{Model: model}) {
				return "", fmt.Errorf("set_model: session %s is not live on this host", cor.SessionID)
			}
			msg := fmt.Sprintf("Switching this session to %q: the rest of this turn finishes on the current model; the next turn runs on the new one.", model)
			if a.Default {
				if setDefault == nil {
					return msg + " Host default NOT saved: this host has no settings store.", nil
				}
				if err := setDefault(model); err != nil {
					return "", fmt.Errorf("set_model: session switch accepted, but saving the host default failed: %w", err)
				}
				msg += " Saved as the host default: new sessions start on it (an exported ARBOS_MODEL still overrides at launch)."
			}
			return msg, nil
		})
	if err := reg.Register(spec, schema); err != nil {
		return err
	}
	if catalog == nil {
		return nil
	}
	listSchema, err := jsonschema.Reflect(reflect.TypeOf(ListArgs{}))
	if err != nil {
		return fmt.Errorf("list_models schema: %w", err)
	}
	listSpec := tool.NewSpec("list_models", listDescription, false,
		func(ctx context.Context, a ListArgs) (string, error) {
			models, err := catalog(ctx)
			if err != nil {
				return "", fmt.Errorf("list_models: catalog fetch failed: %w", err)
			}
			return renderList(models, strings.TrimSpace(a.Query)), nil
		})
	return reg.Register(listSpec, listSchema)
}

// rejectUnlisted returns the refusal message for a model id the catalog does
// not contain — close matches when any exist, a pointer to list_models when
// none do — or "" when the id is listed and the switch should proceed.
func rejectUnlisted(models []modelcatalog.Model, id string) string {
	for _, m := range models {
		if m.ID == id {
			return ""
		}
	}
	matches := rank(models, id)
	if len(matches) > suggestMax {
		matches = matches[:suggestMax]
	}
	if len(matches) == 0 {
		return fmt.Sprintf("Did not switch: %q is not in the provider's catalog and nothing similar matched. Use list_models to find the exact id, or repeat with force:true if the id is correct but newer than the catalog.", id)
	}
	var b strings.Builder
	fmt.Fprintf(&b, "Did not switch: %q is not in the provider's catalog. Close matches:\n", id)
	for _, m := range matches {
		b.WriteString(renderModel(m))
	}
	b.WriteString("Call set_model again with the exact id (or force:true if the id is correct but unlisted).")
	return b.String()
}

// rank returns the catalog entries matching query as a substring of id or
// display name, best first: an id hit beats a name-only hit, an earlier hit
// beats a later one, a shorter id beats a longer — the same ordering the web
// picker uses, so the agent and the user see the same "closest" models.
func rank(models []modelcatalog.Model, query string) []modelcatalog.Model {
	q := strings.ToLower(query)
	type scored struct {
		m modelcatalog.Model
		s int
	}
	var hits []scored
	for _, m := range models {
		s := -1
		if i := strings.Index(strings.ToLower(m.ID), q); i >= 0 {
			s = i
		} else if i := strings.Index(strings.ToLower(m.Name), q); i >= 0 {
			s = 1000 + i
		}
		if s >= 0 {
			hits = append(hits, scored{m, s})
		}
	}
	sort.SliceStable(hits, func(i, j int) bool {
		if hits[i].s != hits[j].s {
			return hits[i].s < hits[j].s
		}
		return len(hits[i].m.ID) < len(hits[j].m.ID)
	})
	out := make([]modelcatalog.Model, len(hits))
	for i, h := range hits {
		out[i] = h.m
	}
	return out
}

// renderList formats the (optionally filtered) catalog for the model, capped
// at listMax with a truncation note.
func renderList(models []modelcatalog.Model, query string) string {
	matched := models
	if query != "" {
		matched = rank(models, query)
	}
	if len(matched) == 0 {
		if query != "" {
			return fmt.Sprintf("No models match %q (catalog has %d). Try a shorter substring or an empty query.", query, len(models))
		}
		return "The provider returned an empty catalog."
	}
	var b strings.Builder
	if query != "" {
		fmt.Fprintf(&b, "%d of %d models match %q:\n", len(matched), len(models), query)
	} else {
		fmt.Fprintf(&b, "%d models available:\n", len(matched))
	}
	shown := matched
	if len(shown) > listMax {
		shown = shown[:listMax]
	}
	for _, m := range shown {
		b.WriteString(renderModel(m))
	}
	if n := len(matched) - len(shown); n > 0 {
		fmt.Fprintf(&b, "… and %d more — narrow with query.\n", n)
	}
	b.WriteString("Pass an id to set_model exactly as listed.")
	return b.String()
}

// renderModel formats one catalog line: the exact id, then the display name
// and context window when present.
func renderModel(m modelcatalog.Model) string {
	line := "- " + m.ID
	if m.Name != "" {
		line += " — " + m.Name
	}
	if m.ContextLength > 0 {
		line += fmt.Sprintf(" (%dk context)", m.ContextLength/1000)
	}
	return line + "\n"
}
