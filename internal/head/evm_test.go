package head

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/decred/dcrd/dcrec/secp256k1/v4"
)

// fakeRPC stands up a JSON-RPC endpoint returning canned results per method, so
// EVM deposit verification is exercised end-to-end without a live chain.
type fakeRPC struct {
	to       string
	valueHex string
	input    string // tx calldata (hex); empty -> "0x"
	status   string // receipt status: "0x1" success
	txBlock  string
	head     string
	chainID  string // eth_chainId hex; empty -> 0x2105 (Base 8453)
	noTx     bool   // getTransactionByHash returns null
}

func (f fakeRPC) server(t *testing.T) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Method string `json:"method"`
		}
		b, _ := io.ReadAll(r.Body)
		_ = json.Unmarshal(b, &req)
		var result any
		switch req.Method {
		case "eth_chainId":
			if result = f.chainID; f.chainID == "" {
				result = "0x2105" // 8453 (Base)
			}
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
	t.Cleanup(srv.Close)
	return srv
}

const usdcAddr = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"

// transferCalldata builds ERC-20 transfer(to, amount) calldata.
func transferCalldata(to string, amount int64) string {
	return "0x" + erc20TransferSelector +
		strings.Repeat("0", 24) + strings.TrimPrefix(strings.ToLower(to), "0x") +
		fmt.Sprintf("%064x", amount)
}

// evmAddress matches the canonical privkey=1 test vector, proving the secp256k1
// + keccak derivation is correct.
func TestEVMAddressVector(t *testing.T) {
	var b [32]byte
	b[31] = 1
	priv := secp256k1.PrivKeyFromBytes(b[:])
	if got := evmAddress(priv.PubKey()); got != "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf" {
		t.Fatalf("evmAddress(privkey=1) = %s, want 0x7e5f4552091a69125d5dfcb7b8c2659029395bdf", got)
	}
}

// Per-account deposit addresses are minted once and stable, and reverse-resolve
// to the owning account.
func TestDepositAddress(t *testing.T) {
	s := testStore(t)
	a := newAccount(t, s)
	addr, err := s.DepositAddress(context.Background(), a)
	if err != nil {
		t.Fatal(err)
	}
	if len(addr) != 42 || !strings.HasPrefix(addr, "0x") {
		t.Fatalf("bad address %q", addr)
	}
	again, _ := s.DepositAddress(context.Background(), a)
	if again != addr {
		t.Fatalf("address not stable: %s vs %s", addr, again)
	}
	owner, err := s.AccountByDepositAddress(context.Background(), strings.ToUpper(addr))
	if err != nil || owner != a {
		t.Fatalf("reverse lookup = %q,%v, want %s", owner, err, a)
	}
	if _, err := s.AccountByDepositAddress(context.Background(), "0xdeadbeef"); err == nil {
		t.Fatal("unknown address should not resolve")
	}
}

// A native transfer is valued at the ETH rate; a USDC transfer at 1 micro per
// base unit. Both report where they landed (dest) for owner attribution.
func TestEVMVerify(t *testing.T) {
	dest := "0x000000000000000000000000000000000000beef"

	t.Run("native eth", func(t *testing.T) {
		srv := fakeRPC{to: dest, valueHex: "0xde0b6b3a7640000", status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
		f := newEVMFunder(srv.URL, "", 8453, MicroFromUSD(3000), 1)
		dep, err := f.verifyDeposit(context.Background(), "0xdead")
		if err != nil {
			t.Fatal(err)
		}
		if dep.dest != dest || dep.micros != 3_000_000_000 || dep.asset != "eth" {
			t.Fatalf("got dest=%s micros=%d asset=%s", dep.dest, dep.micros, dep.asset)
		}
	})

	t.Run("usdc", func(t *testing.T) {
		srv := fakeRPC{to: usdcAddr, input: transferCalldata(dest, 5_000_000), status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
		f := newEVMFunder(srv.URL, usdcAddr, 8453, MicroFromUSD(3000), 1)
		dep, err := f.verifyDeposit(context.Background(), "0xdead")
		if err != nil {
			t.Fatal(err)
		}
		if dep.dest != dest || dep.micros != 5_000_000 || dep.asset != "usdc" {
			t.Fatalf("got dest=%s micros=%d asset=%s", dep.dest, dep.micros, dep.asset)
		}
	})
}

func TestEVMRejects(t *testing.T) {
	dest := "0x000000000000000000000000000000000000beef"
	base := fakeRPC{to: dest, valueHex: "0xde0b6b3a7640000", status: "0x1", txBlock: "0x10", head: "0x12"}
	mk := func(f fakeRPC) *evmFunder { return newEVMFunder(f.server(t).URL, "", 8453, MicroFromUSD(3000), 1) }

	t.Run("no value", func(t *testing.T) {
		f := base
		f.valueHex = "0x0"
		if _, err := mk(f).verifyDeposit(context.Background(), "0xabc"); err == nil {
			t.Fatal("want no-value error")
		}
	})
	t.Run("pending or failed", func(t *testing.T) {
		f := base
		f.status = "0x0"
		if _, err := mk(f).verifyDeposit(context.Background(), "0xabc"); err == nil {
			t.Fatal("want failed-receipt error")
		}
	})
	t.Run("too few confirmations", func(t *testing.T) {
		f := base
		f.txBlock, f.head = "0x12", "0x12" // 1 confirmation
		fn := newEVMFunder(f.server(t).URL, "", 8453, MicroFromUSD(3000), 6)
		if _, err := fn.verifyDeposit(context.Background(), "0xabc"); err == nil || !strings.Contains(err.Error(), "confirmation") {
			t.Fatalf("want confirmation error, got %v", err)
		}
	})
	t.Run("not found", func(t *testing.T) {
		f := base
		f.noTx = true
		if _, err := mk(f).verifyDeposit(context.Background(), "0xabc"); err == nil {
			t.Fatal("want not-found error")
		}
	})
	t.Run("wrong chain", func(t *testing.T) {
		f := base
		f.chainID = "0xaa36a7" // Sepolia, not 8453
		if _, err := mk(f).verifyDeposit(context.Background(), "0xabc"); err == nil || !strings.Contains(err.Error(), "chain") {
			t.Fatalf("want chain-mismatch error, got %v", err)
		}
	})
}

// The handler credits the account that OWNS the destination address, idempotently.
func TestFundEVMHandlerCreditsOwner(t *testing.T) {
	s := testStore(t)
	acct, key := fundedKey(t, s, 0)
	addr, _ := s.DepositAddress(context.Background(), acct) // the account's deposit address
	srv := fakeRPC{to: usdcAddr, input: transferCalldata(addr, 7_500_000), status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
	h := New(Config{Store: s, Upstream: fakeUpstream{body: "{}"},
		EVMRPCURL: srv.URL, EVMUSDCAddr: usdcAddr, EVMChainID: 8453, EVMETHUSDMicro: MicroFromUSD(3000), EVMMinConf: 1})

	txHash := "0x" + strings.Repeat("a", 64)
	for i := 0; i < 3; i++ { // resubmits must not double-credit
		rec := httptest.NewRecorder()
		r := httptest.NewRequest(http.MethodPost, "/v1/fund/evm", strings.NewReader(`{"tx_hash":"`+txHash+`"}`))
		r.Header.Set("Authorization", "Bearer "+key)
		h.handleFundEVM(rec, r)
		if rec.Code != 200 {
			t.Fatalf("fund #%d status=%d: %s", i, rec.Code, rec.Body.String())
		}
	}
	if got := balance(t, s, acct); got != 7_500_000 {
		t.Fatalf("balance = %d, want 7500000 (credited once)", got)
	}
}

// A deposit to an address no account owns is rejected — nothing to attribute.
func TestFundEVMHandlerUnrecognized(t *testing.T) {
	s := testStore(t)
	_, key := fundedKey(t, s, 0)
	srv := fakeRPC{to: usdcAddr, input: transferCalldata("0x000000000000000000000000000000000000beef", 5_000_000),
		status: "0x1", txBlock: "0x10", head: "0x12"}.server(t)
	h := New(Config{Store: s, Upstream: fakeUpstream{body: "{}"},
		EVMRPCURL: srv.URL, EVMUSDCAddr: usdcAddr, EVMChainID: 8453, EVMETHUSDMicro: MicroFromUSD(3000), EVMMinConf: 1})

	rec := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/v1/fund/evm", strings.NewReader(`{"tx_hash":"0x`+strings.Repeat("b", 64)+`"}`))
	r.Header.Set("Authorization", "Bearer "+key)
	h.handleFundEVM(rec, r)
	if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), "deposit_unrecognized") {
		t.Fatalf("want 400 deposit_unrecognized, got %d: %s", rec.Code, rec.Body.String())
	}
}
