package head

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"sync"
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
	rpcURL  string
	usdc    string // lowercased USDC token contract ("" disables USDC deposits)
	chainID int64
	ethUSD  int64 // micro-USD per 1 ETH (1e18 wei): the conversion rate
	minConf int64
	client  *http.Client

	// chainMu guards the one-time chain-id pin. Deposit addresses exist on every
	// EVM chain, so a misconfigured RPC pointed at a testnet would otherwise let
	// free testnet coin be credited at the mainnet rate. We assert eth_chainId
	// == chainID once (cached on success, retried on a transient RPC failure)
	// before trusting any deposit.
	chainMu sync.Mutex
	chainOK bool
}

func newEVMFunder(rpcURL, usdc string, chainID, ethUSDMicro, minConf int64) *evmFunder {
	if minConf < 1 {
		minConf = 1
	}
	return &evmFunder{
		rpcURL:  rpcURL,
		usdc:    strings.ToLower(strings.TrimSpace(usdc)),
		chainID: chainID,
		ethUSD:  ethUSDMicro,
		minConf: minConf,
		// Short, bounded JSON-RPC calls (not streams), so a client timeout is
		// safe here; the pooled transport keeps the RPC connection warm.
		client: &http.Client{Timeout: 20 * time.Second, Transport: pooledTransport()},
	}
}

// erc20TransferSelector is keccak("transfer(address,uint256)")[:4].
const erc20TransferSelector = "a9059cbb"

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

// deposit is a verified, confirmed deposit ready to credit: the destination
// address it landed at (which determines the owning account), the amount in
// micro-USD, and the asset (for the ledger meta).
type deposit struct {
	dest   string // lowercased 0x destination address
	micros int64
	asset  string // "eth" | "usdc"
}

// verifyDeposit confirms txHash is a successful, sufficiently-confirmed transfer
// and returns where it landed and its micro-USD value — either a native coin
// transfer (dest = tx.to) or a USDC transfer (dest = the ERC-20 recipient). The
// caller maps dest to the owning account; an unrecognized dest is rejected
// there. Errors name why a tx does not qualify (no value, pending, too few
// confirmations, wrong chain).
func (e *evmFunder) verifyDeposit(ctx context.Context, txHash string) (deposit, error) {
	if err := e.ensureChain(ctx); err != nil {
		return deposit{}, err
	}
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
	to := strings.ToLower(tx.To)

	// A call to the USDC contract is a token transfer; anything else is treated
	// as a native-coin transfer to its recipient.
	if e.usdc != "" && to == e.usdc {
		recipient, amount, ok := decodeERC20Transfer(tx.Input)
		if !ok {
			return deposit{}, errors.New("transaction is not a token transfer")
		}
		if amount.Sign() <= 0 {
			return deposit{}, errors.New("token transfer carries no value")
		}
		if err := e.confirmed(ctx, txHash); err != nil {
			return deposit{}, err
		}
		// USDC has 6 decimals, so one base unit is exactly one micro-USD.
		if !amount.IsInt64() {
			return deposit{}, errors.New("token transfer amount out of range")
		}
		return deposit{dest: recipient, micros: amount.Int64(), asset: "usdc"}, nil
	}

	wei, ok := new(big.Int).SetString(strings.TrimPrefix(tx.Value, "0x"), 16)
	if !ok || wei.Sign() <= 0 {
		return deposit{}, errors.New("transaction carries no value")
	}
	if err := e.confirmed(ctx, txHash); err != nil {
		return deposit{}, err
	}
	return deposit{dest: to, micros: e.micros(wei), asset: "eth"}, nil
}

// confirmed checks the receipt succeeded and the tx has at least minConf
// confirmations against the current head.
func (e *evmFunder) confirmed(ctx context.Context, txHash string) error {
	var rcpt struct {
		Status      string `json:"status"`
		BlockNumber string `json:"blockNumber"`
	}
	if err := e.rpc(ctx, "eth_getTransactionReceipt", []any{txHash}, &rcpt); err != nil {
		return err
	}
	if rcpt.Status != "0x1" {
		return errors.New("transaction is pending or failed")
	}
	var headHex string
	if err := e.rpc(ctx, "eth_blockNumber", nil, &headHex); err != nil {
		return err
	}
	if conf := hexToInt64(headHex) - hexToInt64(rcpt.BlockNumber) + 1; conf < e.minConf {
		return fmt.Errorf("only %d confirmation(s), need %d", conf, e.minConf)
	}
	return nil
}

// decodeERC20Transfer parses an ERC-20 transfer(address,uint256) calldata,
// returning the lowercased 0x recipient and the amount. ok is false for any
// other calldata shape.
func decodeERC20Transfer(inputHex string) (recipient string, amount *big.Int, ok bool) {
	h := strings.TrimPrefix(strings.ToLower(inputHex), "0x")
	if len(h) < 8+64+64 || h[:8] != erc20TransferSelector {
		return "", nil, false
	}
	recipient = "0x" + h[8+24:8+64] // last 20 bytes of the left-padded address word
	amt, ok2 := new(big.Int).SetString(h[8+64:8+128], 16)
	if !ok2 {
		return "", nil, false
	}
	return recipient, amt, true
}

// ensureChain asserts the RPC endpoint actually serves the configured chain,
// once, before any deposit is trusted. A zero chainID means the operator did
// not pin one, so the check is skipped (best they can do). Success is cached; a
// transient RPC failure is not, so a blip doesn't wedge funding.
func (e *evmFunder) ensureChain(ctx context.Context) error {
	if e.chainID == 0 {
		return nil
	}
	e.chainMu.Lock()
	defer e.chainMu.Unlock()
	if e.chainOK {
		return nil
	}
	var idHex string
	if err := e.rpc(ctx, "eth_chainId", nil, &idHex); err != nil {
		return fmt.Errorf("chain id check: %w", err)
	}
	if got := hexToInt64(idHex); got != e.chainID {
		return fmt.Errorf("configured for chain %d but the RPC serves chain %d", e.chainID, got)
	}
	e.chainOK = true
	return nil
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
	// Authenticated to gate RPC abuse; the credit goes to the address owner
	// (resolved below), not necessarily the caller.
	if _, ok := h.requirePrincipal(w, r); !ok {
		return
	}
	// Guards a direct call (the route is only registered when h.evm != nil, so
	// this is unreachable through the mux but real for tests/embedders).
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
	// Attribute by destination: the deposit is credited to the account that
	// owns the address it landed at — not the caller. No tag, no submitter
	// race; an unrecognized destination is simply not one of our addresses.
	owner, err := h.store.AccountByDepositAddress(r.Context(), dep.dest)
	if err != nil {
		writeAPIErr(w, http.StatusBadRequest, "deposit_unrecognized",
			"transaction did not pay a known arbos deposit address — fetch yours from /v1/fund/info")
		return
	}
	if dep.micros <= 0 {
		writeAPIErr(w, http.StatusBadRequest, "deposit_too_small", "deposit is below one micro-USD")
		return
	}
	ref := fmt.Sprintf("evm:%d:%s", h.evm.chainID, txHash)
	if err := h.store.Credit(r.Context(), owner, dep.micros, reasonFunding, ref, map[string]any{
		"source": "evm", "asset": dep.asset, "chain_id": h.evm.chainID, "tx_hash": txHash, "dest": dep.dest,
	}); err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not credit account")
		return
	}
	bal, _ := h.store.Balance(r.Context(), owner)
	writeJSON(w, http.StatusOK, map[string]any{
		"credited_micro": dep.micros,
		"balance_micro":  bal,
		"account_id":     owner,
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
	if h.evm == nil && !h.mock {
		writeAPIErr(w, http.StatusServiceUnavailable, "unavailable", "crypto funding is not configured on this head")
		return
	}
	addr, err := h.store.DepositAddress(r.Context(), princ.AccountID)
	if err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not derive deposit address")
		return
	}
	info := map[string]any{"address": addr}
	if h.evm != nil {
		info["chain_id"] = h.evm.chainID
		info["usdc"] = h.evm.usdc
		info["note"] = "Send USDC (or native ETH) to your address on this chain, then POST the tx hash to /v1/fund/evm. Funds are credited to the account that owns the address."
	}
	if h.mock {
		info["mock"] = true // the dashboard simulates payment via POST /v1/fund/mock
	}
	writeJSON(w, http.StatusOK, info)
}

// handleFundMock credits the caller's account by a USD amount with no chain or
// wallet — a local demo faucet, registered only in mock mode.
func (h *Head) handleFundMock(w http.ResponseWriter, r *http.Request) {
	princ, ok := h.requirePrincipal(w, r)
	if !ok {
		return
	}
	if !h.mock {
		writeAPIErr(w, http.StatusNotFound, "not_found", "not available")
		return
	}
	var req struct {
		USD float64 `json:"usd"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 1<<16)).Decode(&req); err != nil || req.USD <= 0 {
		writeAPIErr(w, http.StatusBadRequest, "bad_request", "a positive usd amount is required")
		return
	}
	micro := MicroFromUSD(req.USD)
	if err := h.store.Credit(r.Context(), princ.AccountID, micro, reasonFunding, "mock:"+newID("pay"), map[string]any{"source": "mock"}); err != nil {
		writeAPIErr(w, http.StatusInternalServerError, "internal", "could not credit account")
		return
	}
	bal, _ := h.store.Balance(r.Context(), princ.AccountID)
	writeJSON(w, http.StatusOK, map[string]any{"credited_micro": micro, "balance_micro": bal})
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
