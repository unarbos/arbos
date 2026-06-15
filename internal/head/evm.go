package head

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"time"
)

// weiPerETH is 1e18 — the wei in one ETH, the divisor when converting a
// native-coin deposit to micro-USD.
var weiPerETH = new(big.Int).Exp(big.NewInt(10), big.NewInt(18), nil)

// evmFunder credits accounts for confirmed native-coin deposits to the head's
// treasury address, verified directly against an EVM JSON-RPC endpoint with no
// chain library — just eth_getTransactionByHash, eth_getTransactionReceipt, and
// eth_blockNumber. It is the crypto arm of funding: a confirmed on-chain
// deposit becomes a ledger credit, idempotent on chain id + tx hash. ERC-20
// (USDC) deposits are a later addition on the same seam; v1 is native coin.
type evmFunder struct {
	rpcURL   string
	treasury string // lowercased 0x address that must receive the deposit
	chainID  int64
	ethUSD   int64 // micro-USD per 1 ETH (1e18 wei): the conversion rate
	minConf  int64
	client   *http.Client
}

func newEVMFunder(rpcURL, treasury string, chainID, ethUSDMicro, minConf int64) *evmFunder {
	if minConf < 1 {
		minConf = 1
	}
	return &evmFunder{
		rpcURL:   rpcURL,
		treasury: strings.ToLower(strings.TrimSpace(treasury)),
		chainID:  chainID,
		ethUSD:   ethUSDMicro,
		minConf:  minConf,
		client:   &http.Client{Timeout: 20 * time.Second},
	}
}

// rpc issues one JSON-RPC call and unmarshals result into out (when non-nil).
func (e *evmFunder) rpc(ctx context.Context, method string, params []any, out any) error {
	reqBody, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, e.rpcURL, bytes.NewReader(reqBody))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := e.client.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	var env struct {
		Result json.RawMessage `json:"result"`
		Error  *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&env); err != nil {
		return fmt.Errorf("rpc %s: %w", method, err)
	}
	if env.Error != nil {
		return fmt.Errorf("rpc %s: %s", method, env.Error.Message)
	}
	if out != nil {
		return json.Unmarshal(env.Result, out)
	}
	return nil
}

// deposit is a verified treasury deposit: the value moved and the transaction's
// input data (used to bind the deposit to an account).
type deposit struct {
	wei   *big.Int
	input string // raw hex calldata, "0x"-prefixed
}

// verifyDeposit returns the deposit when txHash is a confirmed, successful
// native-coin transfer to the treasury; otherwise an error naming why it does
// not qualify (wrong recipient, no value, pending, too few confirmations).
func (e *evmFunder) verifyDeposit(ctx context.Context, txHash string) (deposit, error) {
	var tx struct {
		To    string `json:"to"`
		Value string `json:"value"`
		Input string `json:"input"`
	}
	if err := e.rpc(ctx, "eth_getTransactionByHash", []any{txHash}, &tx); err != nil {
		return deposit{}, err
	}
	if tx.To == "" {
		return deposit{}, errors.New("transaction not found")
	}
	if strings.ToLower(tx.To) != e.treasury {
		return deposit{}, errors.New("transaction recipient is not the treasury address")
	}
	wei, ok := new(big.Int).SetString(strings.TrimPrefix(tx.Value, "0x"), 16)
	if !ok || wei.Sign() <= 0 {
		return deposit{}, errors.New("transaction carries no value")
	}
	var rcpt struct {
		Status      string `json:"status"`
		BlockNumber string `json:"blockNumber"`
	}
	if err := e.rpc(ctx, "eth_getTransactionReceipt", []any{txHash}, &rcpt); err != nil {
		return deposit{}, err
	}
	if rcpt.Status != "0x1" {
		return deposit{}, errors.New("transaction is pending or failed")
	}
	var headHex string
	if err := e.rpc(ctx, "eth_blockNumber", nil, &headHex); err != nil {
		return deposit{}, err
	}
	conf := hexToInt64(headHex) - hexToInt64(rcpt.BlockNumber) + 1
	if conf < e.minConf {
		return deposit{}, fmt.Errorf("only %d confirmation(s), need %d", conf, e.minConf)
	}
	return deposit{wei: wei, input: tx.Input}, nil
}

// evmDepositData is the hex calldata a depositor must attach so the head can
// attribute the deposit: the account id, UTF-8, hex-encoded.
func evmDepositData(accountID string) string {
	return "0x" + hex.EncodeToString([]byte(accountID))
}

// taggedFor reports whether a deposit's calldata carries accountID. Because the
// sender chooses the calldata, a deposit is bound to exactly one account at
// signing time: another account cannot claim it (its tag would not match), and
// front-running a known tx hash fails the same check. This is the stopgap that
// makes a single shared treasury safe to self-attribute until per-account
// deposit addresses land.
func (d deposit) taggedFor(accountID string) bool {
	raw, err := hex.DecodeString(strings.TrimPrefix(strings.ToLower(d.input), "0x"))
	if err != nil {
		return false
	}
	return strings.Contains(string(raw), accountID)
}

// micros converts a wei amount to micro-USD: wei * (micro-USD per ETH) / 1e18.
func (e *evmFunder) micros(wei *big.Int) int64 {
	m := new(big.Int).Mul(wei, big.NewInt(e.ethUSD))
	m.Div(m, weiPerETH)
	return m.Int64()
}

// handleFundEVM credits the caller's account for a confirmed treasury deposit.
// The caller authenticates with its arbos key and submits the deposit tx hash;
// the head verifies it on-chain and posts the USD-equivalent credit, idempotent
// on chain id + tx hash so a resubmit never double-credits.
func (h *Head) handleFundEVM(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	if h.evm == nil {
		writeAPIErr(w, http.StatusServiceUnavailable, "unavailable", "crypto funding is not configured on this head")
		return
	}
	var req struct {
		TxHash string `json:"tx_hash"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<16)).Decode(&req); err != nil {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "malformed body")
		return
	}
	txHash := strings.ToLower(strings.TrimSpace(req.TxHash))
	if !strings.HasPrefix(txHash, "0x") || len(txHash) != 66 {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "tx_hash must be a 0x-prefixed 32-byte hash")
		return
	}
	dep, err := h.evm.verifyDeposit(r.Context(), txHash)
	if err != nil {
		writeAPIErr(w, http.StatusBadRequest, "deposit_unverified", err.Error())
		return
	}
	// The deposit must be tagged for this account in its calldata, so a
	// confirmed treasury deposit can be credited only to the account it was
	// signed for — no cross-account claim of a shared treasury.
	if !dep.taggedFor(princ.AccountID) {
		writeAPIErr(w, http.StatusBadRequest, "deposit_untagged",
			"deposit must carry your account id in the transaction data — see GET /v1/fund/info")
		return
	}
	micro := h.evm.micros(dep.wei)
	if micro <= 0 {
		writeAPIErr(w, http.StatusBadRequest, "deposit_too_small", "deposit is below one micro-USD")
		return
	}
	ref := fmt.Sprintf("evm:%d:%s", h.evm.chainID, txHash)
	if err := h.store.Credit(r.Context(), princ.AccountID, micro, reasonFunding, ref, map[string]any{
		"source": "evm", "chain_id": h.evm.chainID, "tx_hash": txHash, "wei": dep.wei.String(),
	}); err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not credit account")
		return
	}
	bal, _ := h.store.Balance(r.Context(), princ.AccountID)
	writeJSON(w, http.StatusOK, map[string]any{
		"credited_micro": micro,
		"balance_micro":  bal,
		"tx_hash":        txHash,
		"chain_id":       h.evm.chainID,
	})
}

// handleFundInfo tells the caller exactly how to fund: where to send, on which
// chain, and the account-specific calldata to attach so the deposit attributes
// to them.
func (h *Head) handleFundInfo(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	if h.evm == nil {
		writeAPIErr(w, http.StatusServiceUnavailable, "unavailable", "crypto funding is not configured on this head")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"chain_id":     h.evm.chainID,
		"treasury":     h.evm.treasury,
		"deposit_data": evmDepositData(princ.AccountID),
		"note":         "send native ETH to treasury on this chain with deposit_data as the transaction data, then POST the tx hash to /v1/fund/evm",
	})
}

// hexToInt64 parses a 0x-prefixed hex quantity; a malformed value reads as 0.
func hexToInt64(h string) int64 {
	h = strings.TrimPrefix(strings.TrimPrefix(h, "0x"), "0X")
	if h == "" {
		return 0
	}
	n, ok := new(big.Int).SetString(h, 16)
	if !ok {
		return 0
	}
	return n.Int64()
}
