// Package server implements the HTTP server and deploy handler.
package server

import (
	"context"
	"crypto/subtle"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"dpl/internal/app"

	"gopkg.in/yaml.v3"
)

// entityMeta holds the minimal fields read from any entity's config.yaml,
// enough for auth validation and type-based dispatch.
type entityMeta struct {
	Type   string   `yaml:"type"`
	Tokens []string `yaml:"tokens"`
}

type contextKey string

const metaKey contextKey = "entityMeta"

// loadEntityMeta reads type and tokens from baseDir/name/config.yaml.
func loadEntityMeta(baseDir, name string) (*entityMeta, error) {
	p := filepath.Join(baseDir, name, "config.yaml")
	data, err := os.ReadFile(p)
	if err != nil {
		return nil, fmt.Errorf("load entity meta: %w", err)
	}
	var meta entityMeta
	if err := yaml.Unmarshal(data, &meta); err != nil {
		return nil, fmt.Errorf("load entity meta: %w", err)
	}
	return &meta, nil
}

// newMux builds the HTTP routes for the given base directory.
func newMux(baseDir string) *http.ServeMux {
	mux := http.NewServeMux()
	mux.Handle("POST /deploy/{name}", authMiddleware(baseDir, http.HandlerFunc(makeDeployHandler(baseDir))))
	return mux
}

// authMiddleware validates the Bearer token against the entity's config.
func authMiddleware(baseDir string, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")

		meta, err := loadEntityMeta(baseDir, name)
		if err != nil {
			if errors.Is(err, os.ErrNotExist) {
				http.Error(w, "entity not found", http.StatusNotFound)
				return
			}
			slog.Error("failed to load entity meta", "name", name, "error", err)
			http.Error(w, "internal error", http.StatusInternalServerError)
			return
		}

		token := extractBearer(r.Header.Get("Authorization"))
		if token == "" {
			http.Error(w, "missing token", http.StatusUnauthorized)
			return
		}

		if !matchToken(token, meta.Tokens) {
			http.Error(w, "invalid token", http.StatusUnauthorized)
			return
		}

		ctx := context.WithValue(r.Context(), metaKey, meta)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// extractBearer returns the token from "Bearer <token>", or empty string.
func extractBearer(header string) string {
	const prefix = "Bearer "
	if !strings.HasPrefix(header, prefix) {
		return ""
	}
	return header[len(prefix):]
}

// matchToken checks whether tok matches any of the valid tokens using
// constant-time comparison.
func matchToken(tok string, valid []string) bool {
	tokB := []byte(tok)
	for _, v := range valid {
		if subtle.ConstantTimeCompare(tokB, []byte(v)) == 1 {
			return true
		}
	}
	return false
}

// makeDeployHandler returns the handler for POST /deploy/{name}.
// It dispatches based on entity type and runs the deploy pipeline.
func makeDeployHandler(baseDir string) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		meta := r.Context().Value(metaKey).(*entityMeta)

		switch meta.Type {
		case "app":
			deployApp(w, r, baseDir, name)
		default:
			http.Error(w, fmt.Sprintf("unsupported entity type: %s", meta.Type), http.StatusBadRequest)
		}
	}
}

// deployApp handles the full deploy pipeline for an app entity.
func deployApp(w http.ResponseWriter, r *http.Request, baseDir, name string) {
	cfg, err := app.LoadConfig(filepath.Join(baseDir, name))
	if err != nil {
		if errors.Is(err, app.ErrUnsupportedType) {
			http.Error(w, "unsupported entity type", http.StatusBadRequest)
			return
		}
		slog.Error("load app config", "name", name, "error", err)
		http.Error(w, "failed to load app config", http.StatusInternalServerError)
		return
	}

	deployDir, err := app.Deploy(cfg, name, baseDir, r.Body)
	if err != nil {
		slog.Error("deploy app", "name", name, "error", err)
		http.Error(w, fmt.Sprintf("deploy failed: %v", err), http.StatusInternalServerError)
		return
	}

	slog.Info("deploy completed", "name", name, "dir", deployDir)
	fmt.Fprintf(w, "deployed %s to %s\n", name, deployDir)
}

// Run starts the HTTP server and blocks until it receives SIGINT/SIGTERM.
// baseDir is the root directory containing entity subdirectories (e.g. /opt/dpl).
func Run(baseDir string) error {
	port := os.Getenv("DPL_PORT")
	if port == "" {
		port = "6060"
	}

	addr := net.JoinHostPort("", port)
	srv := &http.Server{
		Addr:    addr,
		Handler: newMux(baseDir),
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	errCh := make(chan error, 1)
	go func() {
		slog.Info("server starting", "addr", addr)
		errCh <- srv.ListenAndServe()
	}()

	select {
	case err := <-errCh:
		if err != nil && err != http.ErrServerClosed {
			return fmt.Errorf("server: %w", err)
		}
	case <-ctx.Done():
		slog.Info("shutting down")
		shutCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := srv.Shutdown(shutCtx); err != nil {
			return fmt.Errorf("shutdown: %w", err)
		}
	}

	return nil
}
