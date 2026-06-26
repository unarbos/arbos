package main

import (
	"context"
	"path/filepath"

	"github.com/unarbos/arbos/internal/matrix"
	"github.com/unarbos/arbos/internal/matrix/hs"
	"maunium.net/go/mautrix"
)

// matrixEnvFlag gates the embedded Matrix substrate (ADR-0041). Off by default,
// so the shipped binary's behavior is unchanged until Matrix is the default; set
// ARBOS_MATRIX=1 to start the in-process homeserver and mirror sessions to rooms.
const matrixEnvFlag = "ARBOS_MATRIX"

// matrixDevPassword is the loopback-only agent account password. The homeserver
// binds 127.0.0.1, so this never crosses the wire; a real per-node credential
// derived from the device key (D3) replaces it when identity wiring lands.
const matrixDevPassword = "arbos-local-dev"

// startEmbeddedMatrix brings up the in-process homeserver under <dataDir>/matrix,
// provisions and logs in the node's agent user, and returns a Mirror that
// publishes Room-audience events into per-session rooms, plus a shutdown hook.
// dataDir is the .arbos directory (alongside sessions.db).
func startEmbeddedMatrix(dataDir string) (matrix.Mirror, func(), error) {
	ctx := context.Background()
	homeserver, err := hs.Start(ctx, filepath.Join(dataDir, "matrix"), "localhost", "127.0.0.1:18008")
	if err != nil {
		return nil, nil, err
	}
	client, err := mautrix.NewClient(homeserver.BaseURL, "", "")
	if err != nil {
		homeserver.Shutdown()
		return nil, nil, err
	}
	// Idempotent across restarts: create-then-login; an existing account just
	// logs in (the create error is benign on a returning node).
	_, _ = homeserver.CreateAccount(ctx, "agent", matrixDevPassword)
	if _, err := client.Login(&mautrix.ReqLogin{
		Type:             "m.login.password",
		Identifier:       mautrix.UserIdentifier{Type: "m.id.user", User: "agent"},
		Password:         matrixDevPassword,
		StoreCredentials: true,
	}); err != nil {
		homeserver.Shutdown()
		return nil, nil, err
	}
	return matrix.NewClientMirror(client), homeserver.Shutdown, nil
}
