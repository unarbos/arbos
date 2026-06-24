package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// fakeAdmin records the multi-provider seam calls so the handlers can be
// exercised without a real settings store / vault.
type fakeAdmin struct {
	added    []ProviderInput
	updated  map[string]ProviderInput
	removed  []string
	activated []string
	applied  int
	failNext error
}

func newFakeServer(a *fakeAdmin) *Server {
	llm := &LLMAdmin{
		Info:  func() LLMInfo { return LLMInfo{} },
		Apply: func() { a.applied++ },
		AddProvider: func(p ProviderInput) error {
			if a.failNext != nil {
				return a.failNext
			}
			a.added = append(a.added, p)
			return nil
		},
		UpdateProvider: func(id string, p ProviderInput) error {
			if a.updated == nil {
				a.updated = map[string]ProviderInput{}
			}
			a.updated[id] = p
			return nil
		},
		RemoveProvider:    func(id string) error { a.removed = append(a.removed, id); return nil },
		SetActiveProvider: func(id string) error { a.activated = append(a.activated, id); return nil },
	}
	return &Server{LLM: llm}
}

func do(t *testing.T, h http.HandlerFunc, method, target, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, target, strings.NewReader(body))
	rec := httptest.NewRecorder()
	h(rec, req)
	return rec
}

func TestAddProviderHandler(t *testing.T) {
	a := &fakeAdmin{}
	s := newFakeServer(a)
	rec := do(t, s.handleProvidersPost, "POST", "/api/llm/providers",
		`{"label":"gm","provider_name":"openai","endpoint":"https://api.saygm.com/v1","key":"sk-x"}`)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("status = %d, body = %s", rec.Code, rec.Body.String())
	}
	if len(a.added) != 1 || a.added[0].Label != "gm" {
		t.Fatalf("AddProvider not called as expected: %+v", a.added)
	}
	if a.added[0].Key == nil || *a.added[0].Key != "sk-x" {
		t.Errorf("key not passed through: %+v", a.added[0].Key)
	}
	if a.applied != 1 {
		t.Errorf("Apply called %d times, want 1", a.applied)
	}
}

func TestAddProviderRejectsBadAdapter(t *testing.T) {
	s := newFakeServer(&fakeAdmin{})
	rec := do(t, s.handleProvidersPost, "POST", "/api/llm/providers",
		`{"provider_name":"cohere","endpoint":"https://x"}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400", rec.Code)
	}
}

func TestAddProviderRejectsBadEndpoint(t *testing.T) {
	s := newFakeServer(&fakeAdmin{})
	rec := do(t, s.handleProvidersPost, "POST", "/api/llm/providers",
		`{"provider_name":"openai","endpoint":"ftp://nope"}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400", rec.Code)
	}
}

func TestActivateAndDeleteHandlers(t *testing.T) {
	a := &fakeAdmin{}
	s := newFakeServer(a)

	req := httptest.NewRequest("POST", "/api/llm/providers/gm/activate", nil)
	req.SetPathValue("id", "gm")
	rec := httptest.NewRecorder()
	s.handleProviderActivate(rec, req)
	if rec.Code != http.StatusNoContent || len(a.activated) != 1 || a.activated[0] != "gm" {
		t.Fatalf("activate failed: code=%d activated=%v", rec.Code, a.activated)
	}

	req = httptest.NewRequest("DELETE", "/api/llm/providers/gm", nil)
	req.SetPathValue("id", "gm")
	rec = httptest.NewRecorder()
	s.handleProviderDelete(rec, req)
	if rec.Code != http.StatusNoContent || len(a.removed) != 1 || a.removed[0] != "gm" {
		t.Fatalf("delete failed: code=%d removed=%v", rec.Code, a.removed)
	}
	if a.applied != 2 {
		t.Errorf("Apply called %d times across activate+delete, want 2", a.applied)
	}
}

// When the seam is nil (a host without multi-provider support), the routes 404
// rather than panicking.
func TestProviderRoutes404WhenUnsupported(t *testing.T) {
	s := &Server{LLM: &LLMAdmin{Info: func() LLMInfo { return LLMInfo{} }}}
	rec := do(t, s.handleProvidersPost, "POST", "/api/llm/providers", `{"provider_name":"openai"}`)
	if rec.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404 when AddProvider is nil", rec.Code)
	}
}
