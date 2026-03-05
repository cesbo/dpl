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
	"os/signal"

	"dpl/internal/base"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"dpl/internal/app"
	"dpl/internal/podman"
	"dpl/internal/systemd"

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
	locker := newEntityLocker()
	mux := http.NewServeMux()
	mux.Handle("POST /deploy/{name}", authMiddleware(baseDir, http.HandlerFunc(makeDeployHandler(baseDir, locker))))
	mux.Handle("GET /deploy/{name}/{deployID}/status", authMiddleware(baseDir, http.HandlerFunc(makeStatusHandler(baseDir))))
	mux.Handle("GET /deploy/{name}/{deployID}/logs", authMiddleware(baseDir, http.HandlerFunc(makeLogsHandler(baseDir))))
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
func makeDeployHandler(baseDir string, locker *entityLocker) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		meta := r.Context().Value(metaKey).(*entityMeta)

		switch meta.Type {
		case "app":
			deployApp(w, r, baseDir, name, locker)
		default:
			http.Error(w, fmt.Sprintf("unsupported entity type: %s", meta.Type), http.StatusBadRequest)
		}
	}
}

// deployApp handles the deploy pipeline for an app entity.
// It acquires a per-entity lock, checks for in-progress deploys (409),
// increments the version, extracts the archive and generates build
// artifacts synchronously, then kicks off `podman build` in a background goroutine.
// Returns 202 Accepted with a JSON deploy_id.
func deployApp(w http.ResponseWriter, r *http.Request, baseDir, name string, locker *entityLocker) {
	entityDir := filepath.Join(baseDir, name)

	cfg, err := app.LoadConfig(entityDir)
	if err != nil {
		if errors.Is(err, app.ErrUnsupportedType) {
			http.Error(w, "unsupported entity type", http.StatusBadRequest)
			return
		}
		slog.Error("load app config", "name", name, "error", err)
		http.Error(w, "failed to load app config", http.StatusInternalServerError)
		return
	}

	// Acquire per-entity lock for version read + check + increment.
	mu := locker.lock(name)
	mu.Lock()

	currentVersion, err := app.ReadVersion(entityDir)
	if err != nil {
		mu.Unlock()
		slog.Error("read version", "name", name, "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	// Check if previous deploy is still building.
	if currentVersion > 0 {
		prevDir := filepath.Join(entityDir, "deploy_"+strconv.Itoa(currentVersion))
		status, _, readErr := app.ReadStatus(prevDir)
		if readErr == nil && status == app.StatusBuilding {
			mu.Unlock()
			http.Error(w, "deploy already in progress", http.StatusConflict)
			return
		}
	}

	newVersion := currentVersion + 1
	if err := app.WriteVersion(entityDir, newVersion); err != nil {
		mu.Unlock()
		slog.Error("write version", "name", name, "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	mu.Unlock()

	result, err := app.Deploy(cfg, name, baseDir, r.Body, newVersion)
	if err != nil {
		slog.Error("deploy app", "name", name, "error", err)
		http.Error(w, fmt.Sprintf("deploy failed: %v", err), http.StatusInternalServerError)
		return
	}

	// Write initial status.
	if err := app.WriteStatus(result.Dir, app.StatusBuilding, ""); err != nil {
		slog.Error("write initial status", "name", name, "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	// Kick off podman build in background.
	tag := podman.ImageTag(name, strconv.Itoa(result.Version))
	secrets := podman.SecretPaths(result.Dir, len(cfg.Build))
	logFile := filepath.Join(result.Dir, "logs", "build.log")

	go func() {
		opts := podman.BuildOpts{
			ContextDir: result.Dir,
			Secrets:    secrets,
			Tag:        tag,
			LogFile:    logFile,
		}
		if err := podman.Build(context.Background(), opts); err != nil {
			slog.Error("podman build failed", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}
		slog.Info("podman build completed", "name", name, "tag", tag)

		// Allocate host port and deploy as a systemd service.
		hostPort, err := systemd.AllocatePort(entityDir)
		if err != nil {
			slog.Error("allocate port", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		serviceContent, err := app.GenerateService(cfg, hostPort, tag)
		if err != nil {
			slog.Error("generate service", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		unit := systemd.ServiceName(name)
		if err := systemd.WriteServiceFile(name, serviceContent); err != nil {
			slog.Error("write service file", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		ctx := context.Background()
		if err := systemd.DaemonReload(ctx); err != nil {
			slog.Error("systemd daemon-reload", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		if err := systemd.Enable(ctx, unit); err != nil {
			slog.Error("systemd enable", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		if err := systemd.Restart(ctx, unit); err != nil {
			slog.Error("systemd restart", "name", name, "error", err)
			app.WriteStatus(result.Dir, app.StatusFailed, err.Error())
			return
		}

		slog.Info("deploy completed", "name", name, "unit", unit, "port", hostPort)
		app.WriteStatus(result.Dir, app.StatusDone, "")
	}()

	// Return 202 with deploy ID.
	deployID := filepath.Base(result.Dir)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(deployResponse{DeployID: deployID})
}

// makeStatusHandler returns the handler for GET /deploy/{name}/{deployID}/status.
func makeStatusHandler(baseDir string) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		deployID := r.PathValue("deployID")

		if !deployIDPattern.MatchString(deployID) {
			http.Error(w, "invalid deploy ID", http.StatusBadRequest)
			return
		}

		deployDir := filepath.Join(baseDir, name, deployID)
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
func makeLogsHandler(baseDir string) func(http.ResponseWriter, *http.Request) {
	return func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")
		deployID := r.PathValue("deployID")

		if !deployIDPattern.MatchString(deployID) {
			http.Error(w, "invalid deploy ID", http.StatusBadRequest)
			return
		}

		logPath := filepath.Join(baseDir, name, deployID, "logs", "build.log")
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

// Run starts the HTTP server and blocks until it receives SIGINT/SIGTERM.
func Run() error {
	addr := net.JoinHostPort("", base.Port)
	srv := &http.Server{
		Addr:    addr,
		Handler: newMux(base.BaseDir),
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
