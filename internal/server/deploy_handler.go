package server

import (
	"encoding/json"
	"fmt"
	"net/http"
	"path/filepath"

	"dpl/internal/entities/app"
)

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
