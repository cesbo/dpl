package server

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"

	"dpl/internal/entities"
	"dpl/internal/entities/app"
)

// deployResponse is the JSON body returned by POST /deploy/{name}.
type deployResponse struct {
	DeployID string `json:"deploy_id"`
}

// deployHandler returns the handler for POST /deploy/{name}.
// It dispatches based on entity type and runs the deploy pipeline.
func (s *httpServer) deployHandler(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	meta := r.Context().Value(metaKey).(*entityMeta)

	switch meta.Type {
	case "app":
		s.deployApp(w, r, name)
	default:
		msg := fmt.Sprintf("unsupported entity type: %s", meta.Type)
		writeError(w, http.StatusBadRequest, msg)
	}
}

// deployApp handles the deploy pipeline for an app entity.
// Returns 202 Accepted with a JSON deploy_id on success,
// or a JSON error with an appropriate HTTP status code.
func (s *httpServer) deployApp(w http.ResponseWriter, r *http.Request, name string) {
	mu := s.locker.lock(name)
	result, err := app.StartDeploy(name, r.Body, mu)
	if err != nil {
		code := http.StatusUnprocessableEntity // 422 by default
		switch {
		case errors.Is(err, entities.ErrConflict):
			code = http.StatusConflict
		case errors.Is(err, os.ErrNotExist):
			code = http.StatusNotFound
		case errors.Is(err, entities.ErrUnsupportedType):
			code = http.StatusBadRequest
		}
		writeError(w, code, err.Error())
		return
	}

	deployID := filepath.Base(result.Dir)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(deployResponse{DeployID: deployID})
}
