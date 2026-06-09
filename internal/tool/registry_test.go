package tool_test

import (
	"context"
	"encoding/json"
	"errors"
	"testing"

	"github.com/unarbos/arbos/internal/core"
	"github.com/unarbos/arbos/internal/tool"
)

func echoSpec() tool.Spec {
	return tool.NewSpec("echo", "echo", true, func(ctx context.Context, a struct {
		Text string `json:"text"`
	}) (string, error) {
		return a.Text, nil
	})
}

func TestRegisterRejectsDuplicate(t *testing.T) {
	r := tool.New()
	if err := r.Register(echoSpec(), json.RawMessage(`{"type":"object"}`)); err != nil {
		t.Fatal(err)
	}
	if err := r.Register(echoSpec(), json.RawMessage(`{"type":"object"}`)); err == nil {
		t.Fatal("expected duplicate registration to error")
	}
}

func TestDispatchDecodesArgsAndReturnsContent(t *testing.T) {
	r := tool.New()
	_ = r.Register(echoSpec(), json.RawMessage(`{"type":"object"}`))
	res := r.Dispatch(context.Background(), core.ToolCall{ID: "1", Name: "echo", Args: json.RawMessage(`{"text":"hi"}`)})
	if res.IsError || res.Content != "hi" {
		t.Fatalf("unexpected result: %+v", res)
	}
}

func TestDispatchHandlerErrorIsData(t *testing.T) {
	r := tool.New()
	_ = r.Register(tool.NewSpec("boom", "", false, func(ctx context.Context, _ struct{}) (string, error) {
		return "", errors.New("kaboom")
	}), json.RawMessage(`{"type":"object"}`))
	res := r.Dispatch(context.Background(), core.ToolCall{ID: "9", Name: "boom"})
	if !res.IsError || res.CallID != "9" || res.Content != "kaboom" {
		t.Fatalf("handler error must surface as error-data with CallID: %+v", res)
	}
}

func TestDispatchPanicIsContained(t *testing.T) {
	r := tool.New()
	_ = r.Register(tool.NewSpec("panic", "", false, func(ctx context.Context, _ struct{}) (string, error) {
		panic("oh no")
	}), json.RawMessage(`{"type":"object"}`))
	res := r.Dispatch(context.Background(), core.ToolCall{ID: "p", Name: "panic"})
	if !res.IsError || res.CallID != "p" {
		t.Fatalf("a tool panic must degrade to an error result, got %+v", res)
	}
}

func TestDispatchBadArgsIsError(t *testing.T) {
	r := tool.New()
	_ = r.Register(echoSpec(), json.RawMessage(`{"type":"object"}`))
	res := r.Dispatch(context.Background(), core.ToolCall{ID: "1", Name: "echo", Args: json.RawMessage(`not json`)})
	if !res.IsError {
		t.Fatal("malformed args must yield an error result")
	}
}
