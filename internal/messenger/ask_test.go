package messenger

import "github.com/unarbos/arbos/internal/core"

import "testing"

func TestAskCallbackRoundTrip(t *testing.T) {
	reqID, optIdx, ok := parseAskCallback(askCallbackData("req-42", 3))
	if !ok || reqID != "req-42" || optIdx != 3 {
		t.Fatalf("round trip: got (%q,%d,%v)", reqID, optIdx, ok)
	}
}

func TestParseAskCallbackRejectsForeignData(t *testing.T) {
	for _, data := range []string{"", "hello", "a|req-1", "b|req-1|0", "a|req-1|x", "a|req-1|-1"} {
		if _, _, ok := parseAskCallback(data); ok {
			t.Errorf("parseAskCallback(%q) accepted a non-ask payload", data)
		}
	}
}

func TestAskKeyboardSingleSelect(t *testing.T) {
	q := core.QuestionRequest{
		RequestID: "req-1",
		Questions: []core.Question{{
			ID:      "color",
			Options: []core.QuestionOption{{ID: "r", Label: "Red"}, {ID: "b", Label: "Blue"}},
		}},
	}
	kb, ok := askKeyboard(q)
	if !ok || len(kb.InlineKeyboard) != 2 {
		t.Fatalf("expected 2 button rows, got ok=%v rows=%d", ok, len(kb.InlineKeyboard))
	}
	if got := kb.InlineKeyboard[1][0].CallbackData; got != "a|req-1|1" {
		t.Errorf("second button data = %q, want a|req-1|1", got)
	}
}

func TestAskKeyboardFallsBackToText(t *testing.T) {
	multiSelect := core.QuestionRequest{Questions: []core.Question{{Options: []core.QuestionOption{{ID: "a"}}, AllowMultiple: true}}}
	if _, ok := askKeyboard(multiSelect); ok {
		t.Error("multi-select should fall back to text, got a keyboard")
	}
	twoQuestions := core.QuestionRequest{Questions: []core.Question{
		{Options: []core.QuestionOption{{ID: "a"}}},
		{Options: []core.QuestionOption{{ID: "b"}}},
	}}
	if _, ok := askKeyboard(twoQuestions); ok {
		t.Error("multiple questions should fall back to text, got a keyboard")
	}
}
