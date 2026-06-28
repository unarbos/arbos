package main

import (
	"context"
	"fmt"
	"net"
	"path/filepath"
	"strings"

	"github.com/unarbos/arbos/internal/matrix"
	"github.com/unarbos/arbos/internal/matrix/hs"
	"maunium.net/go/mautrix"
)

// secretValues sources the plaintext secret values to scrub from federated
// room events. The managed-secret vault satisfies it (EnvValues), kept as a
// seam so the redactor stays testable and the matrix wiring decoupled.
type secretValues interface {
	EnvValues() []string
}

// redactSecretsFn builds the outbound redaction twin (ADR-0041 D12): a function
// that replaces any occurrence of a known managed-secret value with a redaction
// marker before an event is mirrored to its room. Conservative v1 — only the
// vault's env-injected values, and only those long enough to be real secrets
// (an 8-char floor avoids scrubbing common short values like "true"). nil when
// there is no vault, leaving the room copy verbatim.
func redactSecretsFn(vault secretValues) func(string) string {
	if vault == nil {
		return nil
	}
	return func(s string) string {
		for _, kv := range vault.EnvValues() {
			// EnvValues yields "NAME=value"; scrub the value.
			if i := strings.IndexByte(kv, '='); i >= 0 {
				if v := kv[i+1:]; len(v) >= 8 && strings.Contains(s, v) {
					s = strings.ReplaceAll(s, v, "[redacted secret]")
				}
			}
		}
		return s
	}
}

// matrixEnvFlag overrides the embedded Matrix substrate (ADR-0041). Matrix is
// the default substrate for the web door now (the operator runs on a real
// homeserver as @human, the room is the shared mirror), so this is an escape
// hatch, not the switch: ARBOS_MATRIX=0 forces it off (the legacy seam-only
// path), ARBOS_MATRIX=1 forces it on even where it would otherwise be off (a
// one-shot CLI run).
const matrixEnvFlag = "ARBOS_MATRIX"

// matrixDevPassword is the loopback-only agent account password. The homeserver
// binds 127.0.0.1, so this never crosses the wire; a real per-node credential
// derived from the device key (D3) replaces it when identity wiring lands.
const matrixDevPassword = "arbos-local-dev"

// Loopback homeserver coordinates. serverName is the Matrix server_name (the
// machine's forest name later; "localhost" while purely local), bindAddr the
// loopback client-server API listen address. agentUser/humanUser are the two
// resident localparts every node bootstraps (ADR-0041 D9): the autonomous agent
// and the person operating it — distinct Matrix identities so a session room
// has two real members and every event carries a truthful sender.
const (
	matrixServerName = "localhost"
	matrixBindAddr   = "127.0.0.1:18008"
	matrixAgentUser  = "agent"
	matrixHumanUser  = "human"
)

// embeddedMatrix is the live Matrix substrate a started node exposes to its
// adapters: the two logged-in clients (the agent and the person operating it)
// plus the stable coordinates an external Matrix client would dial. The mirror
// publishes each event as whichever of the two actually authored it; the
// gateway bridge resolves rooms and reports both seats.
type embeddedMatrix struct {
	agent         *mautrix.Client
	human         *mautrix.Client
	homeserverURL string
	agentID       string
	humanID       string
	// provision mints+logs in a guest account for a display name (the Bridge
	// uses it to seat shared-chat participants as their own identity).
	provision matrix.GuestProvisioner
}

// startEmbeddedMatrix brings up the in-process homeserver under <dataDir>/matrix
// and provisions/logs in the node's two resident users (agent + person),
// returning the live clients and their coordinates plus a shutdown hook. dataDir
// is the .arbos directory (alongside sessions.db). The caller derives the mirror
// and gateway bridge from the returned clients.
func startEmbeddedMatrix(dataDir string) (*embeddedMatrix, func(), error) {
	ctx := context.Background()
	// Probe the homeserver port before starting Dendrite: its HTTP listener
	// log.Fatals (process exit) on a bind failure, on its own goroutine, so the
	// error would never return here for the caller to degrade on. The most
	// common cause is another arbos node on this machine already holding the
	// fixed loopback port. The probe is closed immediately — a tiny TOCTOU
	// window against Dendrite's own bind, which is acceptable for a guard whose
	// only alternative is a hard crash (ADR-0041 H3).
	if probe, perr := net.Listen("tcp", matrixBindAddr); perr != nil {
		return nil, nil, fmt.Errorf("homeserver port %s unavailable: %w", matrixBindAddr, perr)
	} else {
		_ = probe.Close()
	}
	homeserver, err := hs.Start(ctx, filepath.Join(dataDir, "matrix"), matrixServerName, matrixBindAddr)
	if err != nil {
		return nil, nil, err
	}
	agent, err := loginResident(ctx, homeserver, matrixAgentUser)
	if err != nil {
		homeserver.Shutdown()
		return nil, nil, fmt.Errorf("provision agent identity: %w", err)
	}
	human, err := loginResident(ctx, homeserver, matrixHumanUser)
	if err != nil {
		homeserver.Shutdown()
		return nil, nil, fmt.Errorf("provision person identity: %w", err)
	}
	return &embeddedMatrix{
		agent:         agent,
		human:         human,
		homeserverURL: homeserver.BaseURL,
		agentID:       "@" + matrixAgentUser + ":" + matrixServerName,
		humanID:       "@" + matrixHumanUser + ":" + matrixServerName,
		provision: func(ctx context.Context, label string) (*mautrix.Client, string, error) {
			localpart := matrix.GuestLocalpart(label)
			client, err := loginResident(ctx, homeserver, localpart)
			if err != nil {
				return nil, "", err
			}
			return client, "@" + localpart + ":" + matrixServerName, nil
		},
	}, homeserver.Shutdown, nil
}

// loginResident provisions (idempotently) and logs in one resident localpart on
// the embedded homeserver, returning its client. Create-then-login: an existing
// account just logs in, so the create error is benign on a returning node.
func loginResident(ctx context.Context, homeserver *hs.Homeserver, localpart string) (*mautrix.Client, error) {
	client, err := mautrix.NewClient(homeserver.BaseURL, "", "")
	if err != nil {
		return nil, err
	}
	_, _ = homeserver.CreateAccount(ctx, localpart, matrixDevPassword)
	if _, err := client.Login(&mautrix.ReqLogin{
		Type:             "m.login.password",
		Identifier:       mautrix.UserIdentifier{Type: "m.id.user", User: localpart},
		Password:         matrixDevPassword,
		StoreCredentials: true,
	}); err != nil {
		return nil, err
	}
	return client, nil
}
