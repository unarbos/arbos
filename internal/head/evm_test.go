package head

import (
	"context"
	"encoding/json"
	"io"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// fakeRPC stands up a JSON-RPC endpoint returning canned results per method, so
// the EVM deposit verification is exercised end-to-end without a live chain.
type fakeRPC struct {
	to       string
	valueHex string
	input    string // tx calldata (hex); empty -> "0x"
	status   string // receipt status: "0x1" success
	txBlock  string
	head     string
	noTx     bool // getTransactionByHash returns null
}

func (f fakeRPC) server(t *testing.T) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Method string `json:"method"`
		}
		b, _ := io.ReadAll(r.Body)
		_ = json.Unmarshal(b, &req)
		var result any
		switch req.Method {
		case "eth_getTransactionByHash":
			if f.noTx {
				result = nil
			} else {
				in := f.input
				if in == "" {
					in = "0x"
				}
				result = map[string]any{"to": f.to, "value": f.valueHex, "input": in}
			}
		case "eth_getTransactionReceipt":
			result = map[string]any{"status": f.status, "blockNumber": f.txBlock}
		case "eth_blockNumber":
			result = f.head
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"jsonrpc": "2.0", "id": 1, "result": result})
	}))
}

const treasuryAddr = "0x7f9838641f977b2afe41622fc301edbee31288ad"

// 1 ETH at $3000 credits exactly $3000 (3e9 micro), and verification accepts a
// confirmed transfer to the treasury.
func TestEVMVerifyAndConvert(t *testing.T) {
	srv := fakeRPC{
		to:       treasuryAddr,
		valueHex: "0xde0b6b3a7640000", // 1e18 wei = 1 ETH
		status:   "0x1",
		txBlock:  "0x10",
		head:     "0x12", // 3 confirmations
	}.server(t)
	defer srv.Close()

	f := newEVMFunder(srv.URL, treasuryAddr, 8453, MicroFromUSD(3000), 1)
	dep, err := f.verifyDeposit(context.Background(), "0xdead")
	if err != nil {
		t.Fatalf("verify: %v", err)
	}
	if dep.wei.Cmp(new(big.Int).SetUint64(1_000_000_000_000_000_000)) != 0 {
		t.Fatalf("wei = %s, want 1e18", dep.wei)
	}
	if got := f.micros(dep.wei); got != 3_000_000_000 {
		t.Fatalf("micros = %d, want 3000000000 ($3000)", got)
	}
}

// A deposit is bound to the account named in its calldata: only that account's
// tag matches, so no other account can claim it.
func TestDepositTagging(t *testing.T) {
	acct := "acct_abc123"
	d := deposit{input: evmDepositData(acct)}
	if !d.taggedFor(acct) {
		t.Fatal("deposit should be tagged for its own account")
	}
	if d.taggedFor("acct_other") {
		t.Fatal("deposit must not match a different account (cross-account claim)")
	}
	if (deposit{input: "0x"}).taggedFor(acct) {
		t.Fatal("an untagged deposit must not match any account")
	}
}

func TestEVMRejects(t *testing.T) {
	base := fakeRPC{to: treasuryAddr, valueHex: "0xde0b6b3a7640000", status: "0x1", txBlock: "0x10", head: "0x12"}

	t.Run("wrong recipient", func(t *testing.T) {
		f := base
		f.to = "0x000000000000000000000000000000000000dead"
		srv := f.server(t)
		defer srv.Close()
		fn := newEVMFunder(srv.URL, treasuryAddr, 8453, MicroFromUSD(3000), 1)
		if _, err := fn.verifyDeposit(context.Background(), "0xabc"); err == nil || !strings.Contains(err.Error(), "treasury") {
			t.Fatalf("want treasury-mismatch error, got %v", err)
		}
	})

	t.Run("pending or failed", func(t *testing.T) {
		f := base
		f.status = "0x0"
		srv := f.server(t)
		defer srv.Close()
		fn := newEVMFunder(srv.URL, treasuryAddr, 8453, MicroFromUSD(3000), 1)
		if _, err := fn.verifyDeposit(context.Background(), "0xabc"); err == nil {
			t.Fatal("want error for failed receipt")
		}
	})

	t.Run("too few confirmations", func(t *testing.T) {
		f := base
		f.txBlock = "0x12"
		f.head = "0x12" // 1 confirmation
		srv := f.server(t)
		defer srv.Close()
		fn := newEVMFunder(srv.URL, treasuryAddr, 8453, MicroFromUSD(3000), 6)
		if _, err := fn.verifyDeposit(context.Background(), "0xabc"); err == nil || !strings.Contains(err.Error(), "confirmation") {
			t.Fatalf("want confirmation error, got %v", err)
		}
	})

	t.Run("not found", func(t *testing.T) {
		f := base
		f.noTx = true
		srv := f.server(t)
		defer srv.Close()
		fn := newEVMFunder(srv.URL, treasuryAddr, 8453, MicroFromUSD(3000), 1)
		if _, err := fn.verifyDeposit(context.Background(), "0xabc"); err == nil {
			t.Fatal("want not-found error")
		}
	})
}

// The end-to-end fund handler verifies a deposit tagged for the account and
// posts an idempotent credit; resubmits never double-credit.
func TestFundEVMHandlerCreditsOnce(t *testing.T) {
	s := testStore(t)
	acct, key := fundedKey(t, s, 0)
	srv := fakeRPC{to: treasuryAddr, valueHex: "0xde0b6b3a7640000", input: evmDepositData(acct),
		status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
	defer srv.Close()
	h := New(Config{Store: s, Upstream: fakeUpstream{body: "{}"},
		EVMRPCURL: srv.URL, EVMTreasury: treasuryAddr, EVMChainID: 8453, EVMETHUSDMicro: MicroFromUSD(3000), EVMMinConf: 1})

	txHash := "0x" + strings.Repeat("a", 64)
	fund := func() *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		r := httptest.NewRequest(http.MethodPost, "/v1/fund/evm", strings.NewReader(`{"tx_hash":"`+txHash+`"}`))
		r.Header.Set("Authorization", "Bearer "+key)
		h.handleFundEVM(rec, r)
		return rec
	}
	for i := 0; i < 3; i++ { // resubmits must not double-credit
		if rec := fund(); rec.Code != 200 {
			t.Fatalf("fund #%d status = %d: %s", i, rec.Code, rec.Body.String())
		}
	}
	if got := balance(t, s, acct); got != 3_000_000_000 {
		t.Fatalf("balance = %d, want 3000000000 (credited once)", got)
	}
}

// A deposit tagged for a different account is rejected — the cross-account
// claim guard, exercised through the real handler.
func TestFundEVMHandlerRejectsUntagged(t *testing.T) {
	s := testStore(t)
	_, key := fundedKey(t, s, 0)
	srv := fakeRPC{to: treasuryAddr, valueHex: "0xde0b6b3a7640000", input: evmDepositData("acct_someoneelse"),
		status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
	defer srv.Close()
	h := New(Config{Store: s, Upstream: fakeUpstream{body: "{}"},
		EVMRPCURL: srv.URL, EVMTreasury: treasuryAddr, EVMChainID: 8453, EVMETHUSDMicro: MicroFromUSD(3000), EVMMinConf: 1})

	rec := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/v1/fund/evm", strings.NewReader(`{"tx_hash":"0x`+strings.Repeat("b", 64)+`"}`))
	r.Header.Set("Authorization", "Bearer "+key)
	h.handleFundEVM(rec, r)
	if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "deposit_untagged") {
		t.Fatalf("want 400 deposit_untagged, got %d: %s", rec.Code, rec.Body.String())
	}
}
