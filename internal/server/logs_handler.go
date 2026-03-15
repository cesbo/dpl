package server

import (
	"errors"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strconv"

	"dpl/internal/base"
)

// logsHandler returns the handler for GET /deploy/{name}/{deployID}/logs.
// Supports ?offset=N query parameter to read from a byte offset.
// Returns the log content and an X-Offset header with the new offset.
func (s *httpServer) logsHandler(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	deployID := r.PathValue("deployID")

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
