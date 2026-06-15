package head

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// fakeUpstream returns a canned response, so the gateway's metering/settlement
// path is exercised without a live OpenRouter (the seam Config.Upstream opens).
type fakeUpstream struct {
	body   string
	ctype  string
	status int
}

func (f fakeUpstream) Forward(_ context.Context, _ []byte, _ bool) (*http.Response, error) {
	ct := f.ctype
	if ct == "" {
		ct = "application/json"
	}
	st := f.status
	if st == 0 {
		st = http.StatusOK
	}
	return &http.Response{
		StatusCode: st,
		Header:     http.Header{"Content-Type": {ct}},
		Body:       io.NopCloser(strings.NewReader(f.body)),
	}, nil
}

func (f fakeUpstream) Models(context.Context) (*http.Response, error) {
	return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader("{}"))}, nil
}
func (f fakeUpstream) Name() string { return "fake" }

// fundedKey mints a key on a fresh account seeded with micro balance; returns
// the account id and the secret.
func fundedKey(t *testing.T, s *Store, micro int64) (string, string) {
	t.Helper()
	a := newAccount(t, s)
	_, secret, err := s.MintKey(context.Background(), a, "test")
	if err != nil {
		t.Fatalf("mint: %v", err)
	}
	if micro > 0 {
		if err := s.Credit(context.Background(), a, micro, reasonFunding, "seed:"+a, nil); err != nil {
			t.Fatalf("seed: %v", err)
		}
	}
	return a, secret
}

func chatReq(key, body string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, "/v1/chat/completions", strings.NewReader(body))
	r.Header.Set("Authorization", "Bearer "+key)
	return r
}

// A non-streaming completion is relayed to the client and metered: the balance
// drops by round(upstream cost * 1e6).
func TestGatewayNonStreamMeters(t *testing.T) {
	s := testStore(t)
	acct, key := fundedKey(t, s, 1_000_000)
	resp := `{"id":"gen-1","model":"m","choices":[{"message":{"content":"hi"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.000123}}`
	h := New(Config{Store: s, Upstream: fakeUpstream{body: resp}})

	rec := httptest.NewRecorder()
	h.handleChatCompletions(rec, chatReq(key, `{"model":"m","messages":[]}`))

	if rec.Code != 200 {
		t.Fatalf("status = %d, want 200", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), `"content":"hi"`) {
		t.Fatalf("reply not relayed: %s", rec.Body.String())
	}
	if got := balance(t, s, acct); got != 1_000_000-123 {
		t.Fatalf("balance = %d, want %d (debited 123)", got, 1_000_000-123)
	}
}

// A streaming completion is relayed and metered from the final usage frame.
func TestGatewayStreamMeters(t *testing.T) {
	s := testStore(t)
	acct, key := fundedKey(t, s, 1_000_000)
	stream := "data: {\"id\":\"gen-1\",\"model\":\"m\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n" +
		"data: {\"id\":\"gen-1\",\"model\":\"m\",\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"cost\":0.000077}}\n\n" +
		"data: [DONE]\n\n"
	h := New(Config{Store: s, Upstream: fakeUpstream{body: stream, ctype: "text/event-stream"}})

	rec := httptest.NewRecorder()
	h.handleChatCompletions(rec, chatReq(key, `{"model":"m","stream":true,"messages":[]}`))

	if rec.Code != 200 {
		t.Fatalf("status = %d, want 200", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), "data:") {
		t.Fatalf("stream not relayed")
	}
	if got := balance(t, s, acct); got != 1_000_000-77 {
		t.Fatalf("balance = %d, want %d (debited 77)", got, 1_000_000-77)
	}
}

// A non-positive balance is rejected with 402 before any upstream call.
func TestGatewayBalanceGate(t *testing.T) {
	s := testStore(t)
	_, key := fundedKey(t, s, 0)
	h := New(Config{Store: s, Upstream: fakeUpstream{body: `{}`}})

	rec := httptest.NewRecorder()
	h.handleChatCompletions(rec, chatReq(key, `{"model":"m","messages":[]}`))

	if rec.Code != http.StatusPaymentRequired {
		t.Fatalf("status = %d, want 402", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), "quota_exhausted") {
		t.Fatalf("missing quota_exhausted: %s", rec.Body.String())
	}
}

// An absent or unknown key is rejected with 401 and never reaches the upstream.
func TestGatewayUnauthorized(t *testing.T) {
	s := testStore(t)
	h := New(Config{Store: s, Upstream: fakeUpstream{body: `{}`}})

	rec := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/v1/chat/completions", strings.NewReader(`{"model":"m","messages":[]}`))
	h.handleChatCompletions(rec, r)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("no key: status = %d, want 401", rec.Code)
	}

	rec = httptest.NewRecorder()
	h.handleChatCompletions(rec, chatReq("sk-arbos-nope", `{"model":"m","messages":[]}`))
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("bad key: status = %d, want 401", rec.Code)
	}
}

// priceWithMargin marks the upstream cost up by the configured basis points;
// zero is a pure pass-through.
func TestPriceWithMargin(t *testing.T) {
	cases := []struct {
		bps      int
		upstream int64
		want     int64
	}{
		{0, 1_000_000, 1_000_000},    // pass-through
		{1000, 1_000_000, 1_100_000}, // +10%
		{250, 1_000_000, 1_025_000},  // +2.5%
		{10000, 2_000, 4_000},        // +100%
	}
	for _, c := range cases {
		h := New(Config{Store: testStore(t), Upstream: fakeUpstream{}, MarginBps: c.bps})
		if got := h.priceWithMargin(c.upstream); got != c.want {
			t.Errorf("priceWithMargin(bps=%d, %d) = %d, want %d", c.bps, c.upstream, got, c.want)
		}
	}
}

// The margin is actually applied on the metered debit, end to end.
func TestGatewayAppliesMargin(t *testing.T) {
	s := testStore(t)
	acct, key := fundedKey(t, s, 1_000_000)
	resp := `{"id":"g","model":"m","choices":[{"message":{"content":"hi"}}],"usage":{"cost":0.0001}}` // 100 micro upstream
	h := New(Config{Store: s, Upstream: fakeUpstream{body: resp}, MarginBps: 5000})                   // +50% -> 150
	rec := httptest.NewRecorder()
	h.handleChatCompletions(rec, chatReq(key, `{"model":"m","messages":[]}`))
	if got := balance(t, s, acct); got != 1_000_000-150 {
		t.Fatalf("balance = %d, want %d (debited 150 with 50%% margin)", got, 1_000_000-150)
	}
}

// handleAdminCredit funds an account behind the admin token and rejects a wrong
// or missing token.
func TestAdminCredit(t *testing.T) {
	s := testStore(t)
	acct := newAccount(t, s)
	h := New(Config{Store: s, Upstream: fakeUpstream{}, AdminToken: "secret"})

	credit := func(token, body string) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		r := httptest.NewRequest(http.MethodPost, "/v1/admin/credit", strings.NewReader(body))
		if token != "" {
			r.Header.Set("X-Admin-Token", token)
		}
		h.handleAdminCredit(rec, r)
		return rec
	}

	if rec := credit("", `{"account_id":"`+acct+`","usd":5}`); rec.Code != http.StatusUnauthorized {
		t.Fatalf("no token: status = %d, want 401", rec.Code)
	}
	if rec := credit("wrong", `{"account_id":"`+acct+`","usd":5}`); rec.Code != http.StatusUnauthorized {
		t.Fatalf("wrong token: status = %d, want 401", rec.Code)
	}
	if rec := credit("secret", `{"account_id":"`+acct+`","usd":5,"ref":"r1"}`); rec.Code != 200 {
		t.Fatalf("good token: status = %d: %s", rec.Code, rec.Body.String())
	}
	if got := balance(t, s, acct); got != 5_000_000 {
		t.Fatalf("balance = %d, want 5000000", got)
	}
	// Idempotent on ref.
	credit("secret", `{"account_id":"`+acct+`","usd":5,"ref":"r1"}`)
	if got := balance(t, s, acct); got != 5_000_000 {
		t.Fatalf("balance = %d after replay, want 5000000 (idempotent)", got)
	}
}

// prepareBody turns on usage accounting and detects streaming without dropping
// the caller's fields.
func TestPrepareBody(t *testing.T) {
	body, stream, err := prepareBody([]byte(`{"model":"m","stream":true,"messages":[{"role":"user","content":"x"}]}`))
	if err != nil {
		t.Fatal(err)
	}
	if !stream {
		t.Fatal("stream not detected")
	}
	s := string(body)
	for _, want := range []string{`"usage"`, `"include":true`, `"stream_options"`, `"include_usage":true`, `"model":"m"`, `"content":"x"`} {
		if !strings.Contains(s, want) {
			t.Errorf("rewritten body missing %s: %s", want, s)
		}
	}
	if _, _, err := prepareBody([]byte(`not json`)); err == nil {
		t.Fatal("expected error for non-object body")
	}
	// `null` is valid JSON but unmarshals to a nil map; it must be rejected,
	// not panic on the usage assignment.
	for _, bad := range []string{`null`, ` `, `123`, `"x"`, `[]`} {
		if _, _, err := prepareBody([]byte(bad)); err == nil {
			t.Errorf("prepareBody(%q) = nil error, want rejection", bad)
		}
	}
}
