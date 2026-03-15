package server

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strconv"

	"dpl/internal/base"
	"dpl/internal/entities/app"
)

// deployIDPattern validates deploy IDs: deploy_ followed by one or more digits.
var deployIDPattern = regexp.MustCompile(`^deploy_\d+$`)

// deployHandler returns the handler for POST /deploy/{name}.
// It dispatches based on entity type and runs the deploy pipeline.
func (s *httpServer) deployHandler(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	meta := r.Context().Value(metaKey).(*entityMeta)

	switch meta.Type {
	case "app":
		s.deployApp(w, r, name)
	default:
		http.Error(w, fmt.Sprintf("unsupported entity type: %s", meta.Type), http.StatusBadRequest)
	}
}

// deployApp handles the deploy pipeline for an app entity.
// Returns 202 Accepted with a JSON deploy_id on success,
// or a JSON error with an appropriate HTTP status code.
func (s *httpServer) deployApp(w http.ResponseWriter, r *http.Request, name string) {
	mu := s.locker.lock(name)
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

// statusHandler for GET /deploy/{name}/{deployID}/status.
func (s *httpServer) statusHandler(w http.ResponseWriter, r *http.Request) {
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

// logsHandler returns the handler for GET /deploy/{name}/{deployID}/logs.
// Supports ?offset=N query parameter to read from a byte offset.
// Returns the log content and an X-Offset header with the new offset.
func (s *httpServer) logsHandler(w http.ResponseWriter, r *http.Request) {
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

	info, err := f.Stat()
	if err != nil {
		slog.Error("stat log file", "name", name, "deployID", deployID, "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}

	remaining := info.Size() - offset
	if remaining < 0 {
		remaining = 0
	}

	w.Header().Set("Content-Type", "text/plain")
	w.Header().Set("X-Offset", strconv.FormatInt(offset+remaining, 10))

	if _, err := io.Copy(w, io.LimitReader(f, remaining)); err != nil {
		slog.Error("read log", "name", name, "deployID", deployID, "error", err)
		return
	}
}