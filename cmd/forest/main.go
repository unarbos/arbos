// Command forest runs a forest head (ADR-0034): the registry + relay that
// arbos nodes join with --forest. Self-hosting is the point — point a
// wildcard domain (or an sslip.io name) at the box and run:
//
//	forest --addr :8080 --domain 204-12-163-231.sslip.io:8080
//
// Nodes then join with: arbos --web :8420 --forest http://204-12-163-231.sslip.io:8080
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/unarbos/arbos/internal/forest"
)

func main() {
	addr := flag.String("addr", ":8080", "listen address")
	domain := flag.String("domain", "", "public apex the head answers as, with port if non-default (required)")
	scheme := flag.String("scheme", "", "scheme for minted lease URLs (default: https with --tls-cert, else http)")
	tlsCert := flag.String("tls-cert", "", "TLS certificate (full chain) covering the apex and *.apex")
	tlsKey := flag.String("tls-key", "", "TLS private key")
	state := flag.String("state", defaultStatePath(), "device registry persistence file")
	leaseTTL := flag.Duration("lease-ttl", time.Hour, "how long a lease outlives its last heartbeat")
	heartbeat := flag.Duration("heartbeat", 30*time.Second, "heartbeat cadence handed to nodes")
	flag.Parse()

	if *domain == "" {
		fmt.Fprintln(os.Stderr, "forest: --domain is required (e.g. forest.example.com)")
		os.Exit(2)
	}
	if (*tlsCert == "") != (*tlsKey == "") {
		fmt.Fprintln(os.Stderr, "forest: --tls-cert and --tls-key go together")
		os.Exit(2)
	}
	if *scheme == "" {
		*scheme = "http"
		if *tlsCert != "" {
			*scheme = "https"
		}
	}

	head, err := forest.NewHead(forest.HeadConfig{
		Domain:    *domain,
		Scheme:    *scheme,
		StatePath: *state,
		LeaseTTL:  *leaseTTL,
		Heartbeat: *heartbeat,
		Logf: func(format string, args ...any) {
			fmt.Fprintf(os.Stderr, format+"\n", args...)
		},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "forest: %v\n", err)
		os.Exit(1)
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	go head.Sweep(ctx)

	srv := &http.Server{Addr: *addr, Handler: head.Handler()}
	errCh := make(chan error, 1)
	go func() {
		if *tlsCert != "" {
			errCh <- srv.ListenAndServeTLS(*tlsCert, *tlsKey)
			return
		}
		errCh <- srv.ListenAndServe()
	}()
	fmt.Fprintf(os.Stderr, "forest head for %s://%s listening on %s\n", *scheme, *domain, *addr)

	select {
	case <-ctx.Done():
		shutCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutCtx)
	case err := <-errCh:
		if !errors.Is(err, http.ErrServerClosed) {
			fmt.Fprintf(os.Stderr, "forest: %v\n", err)
			os.Exit(1)
		}
	}
}

func defaultStatePath() string {
	if h, err := os.UserHomeDir(); err == nil {
		return filepath.Join(h, ".config", "arbos-forest", "devices.json")
	}
	return "devices.json"
}
