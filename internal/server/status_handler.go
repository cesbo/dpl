package server

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"

	"dpl/internal/base"
	"dpl/internal/entities/app"
)

// statusResponse is the JSON body returned by GET /deploy/{name}/{deployID}/status.
type statusResponse struct {
	DeployID string `json:"deploy_id"`
	Status   string `json:"status"`
	Error    string `json:"error,omitempty"`
}

// statusHandler for GET /deploy/{name}/{deployID}/status.
func (s *httpServer) statusHandler(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	deployID := r.PathValue("deployID")

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
