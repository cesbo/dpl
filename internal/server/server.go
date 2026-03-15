// Package server implements the HTTP server and deploy handler.
package server

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"dpl/internal/base"
	"dpl/internal/entities"
	"dpl/internal/entities/app"

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
func loadEntityMeta(name string) (*entityMeta, error) {
	p := filepath.Join(base.BaseDir, name, "config.yaml")
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
func newMux() *http.ServeMux {
	locker := newEntityLocker()
	mux := http.NewServeMux()
	mux.Handle("POST /deploy/{name}", authMiddleware(http.HandlerFunc(makeDeployHandler(locker))))
	mux.Handle("GET /deploy/{name}/{deployID}/status", authMiddleware(http.HandlerFunc(makeStatusHandler())))
	mux.Handle("GET /deploy/{name}/{deployID}/logs", authMiddleware(http.HandlerFunc(makeLogsHandler())))
	return mux
}

// authMiddleware validates the Bearer token against the entity's config.
func authMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")

		meta, err := loadEntityMeta(name)
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

// deployIDPattern validates deploy IDs: deploy_ followed by one or more digits.
var deployIDPattern = regexp.MustCompile(`^deploy_\d+$`)

// entityLocker provides per-entity mutual exclusion for deploy operations.
// It protects version read + check + increment to prevent concurrent deploys.
type entityLocker struct {
	mu    sync.Mutex
	locks map[string]*sync.Mutex
}

func newEntityLocker() *entityLocker {
	return &entityLocker{locks: make(map[string]*sync.Mutex)}
}

// lock returns the mutex for the given entity name, creating it if needed.
func (l *entityLocker) lock(name string) *sync.Mutex {
	l.mu.Lock()
	defer l.mu.Unlock()
	m, ok := l.locks[name]
	if !ok {
		m = &sync.Mutex{}
		l.locks[name] = m
	}
	return m
}

// deployResponse is the JSON body returned by POST /deploy/{name}.
type deployResponse struct {
	DeployID string `json:"deploy_id"`
}

// statusResponse is the JSON body returned by GET /deploy/{name}/{deployID}/status.
type statusResponse struct {
	DeployID string `json:"deploy_id"`
	Status   string `json:"status"`
	Error    string `json:"error,omitempty"`
}

// makeDeployHandler returns the handler for POST /deploy/{name}.
// It dispatches based on entity type and runs the deploy pipeline.
func makeDeployHandler(locker *entityLocker) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		meta := r.Context().Value(metaKey).(*entityMeta)

		switch meta.Type {
		case "app":
			deployApp(w, r, name, locker)
		default:
			http.Error(w, fmt.Sprintf("unsupported entity type: %s", meta.Type), http.StatusBadRequest)
		}
	}
}

// errorResponse is the JSON body returned on deploy errors.
type errorResponse struct {
	Error string `json:"error"`
}

// writeError maps an error to an HTTP status code and writes a JSON error response.
func writeError(w http.ResponseWriter, err error) {
	code := http.StatusUnprocessableEntity // 422 by default
	switch {
	case errors.Is(err, entities.ErrConflict):
		code = http.StatusConflict // 409
	case errors.Is(err, os.ErrNotExist):
		code = http.StatusNotFound // 404
	case errors.Is(err, entities.ErrUnsupportedType):
		code = http.StatusBadRequest // 400
	}
	slog.Error("deploy failed", "error", err)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(errorResponse{Error: err.Error()})
}

// deployApp handles the deploy pipeline for an app entity.
// Returns 202 Accepted with a JSON deploy_id on success,
// or a JSON error with an appropriate HTTP status code.
func deployApp(w http.ResponseWriter, r *http.Request, name string, locker *entityLocker) {
	mu := locker.lock(name)
	result, err := app.StartDeploy(name, r.Body, mu)
	if err != nil {
		writeError(w, err)
		return
	}

	deployID := filepath.Base(result.Dir)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(deployResponse{DeployID: deployID})
}

// makeStatusHandler returns the handler for GET /deploy/{name}/{deployID}/status.
func makeStatusHandler() func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		deployID := r.PathValue("deployID")

		if !deployIDPattern.MatchString(deployID) {
			http.Error(w, "invalid deploy ID", http.StatusBadRequest)
			return
		}

		deployDir := filepath.Join(base.BaseDir, name, deployID)
		status, errMsg, err := app.ReadStatus(deployDir)
		if err != nil {
			if errors.Is(err, os.ErrNotExist) {
				http.Error(w, "deploy not found", http.StatusNotFound)
				return
			}
			slog.Error("read deploy status", "name", name, "deployID", deployID, "error", err)
			http.Error(w, "internal error", http.StatusInternalServerError)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(statusResponse{
			DeployID: deployID,
			Status:   status,
			Error:    errMsg,
		})
	}
}

// makeLogsHandler returns the handler for GET /deploy/{name}/{deployID}/logs.
// Supports ?offset=N query parameter to read from a byte offset.
// Returns the log content and an X-Offset header with the new offset.
func makeLogsHandler() func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		deployID := r.PathValue("deployID")

		if !deployIDPattern.MatchString(deployID) {
			http.Error(w, "invalid deploy ID", http.StatusBadRequest)
			return
		}

		logPath := filepath.Join(base.BaseDir, name, deployID, "logs", "build.log")
		f, err := os.Open(logPath)
		if err != nil {
			if errors.Is(err, os.ErrNotExist) {
				http.Error(w, "log not found", http.StatusNotFound)
				return
			}
			slog.Error("open build log", "name", name, "deployID", deployID, "error", err)
			http.Error(w, "internal error", http.StatusInternalServerError)
			return
		}
		defer f.Close()

		var offset int64
		if s := r.URL.Query().Get("offset"); s != "" {
			o, err := strconv.ParseInt(s, 10, 64)
			if err != nil || o < 0 {
				http.Error(w, "invalid offset", http.StatusBadRequest)
				return
			}
			offset = o
		}

		if offset > 0 {
			if _, err := f.Seek(offset, io.SeekStart); err != nil {
				slog.Error("seek log file", "name", name, "deployID", deployID, "error", err)
				http.Error(w, "internal error", http.StatusInternalServerError)
				return
			}
		}

		w.Header().Set("Content-Type", "text/plain")

		data, err := io.ReadAll(f)
		if err != nil {
			slog.Error("read log", "name", name, "deployID", deployID, "error", err)
			http.Error(w, "internal error", http.StatusInternalServerError)
			return
		}

		w.Header().Set("X-Offset", strconv.FormatInt(offset+int64(len(data)), 10))
		w.Write(data)
	}
}

// Run starts the HTTP server and blocks until the context is cancelled.
func Run(ctx context.Context) error {
	addr := net.JoinHostPort("", base.Port)
	srv := &http.Server{
		ReadHeaderTimeout: 5 * time.Second,
		IdleTimeout:       60 * time.Second,
		MaxHeaderBytes:    8 * 1024,
		Addr:              addr,
		Handler:           newMux(),
	}

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
