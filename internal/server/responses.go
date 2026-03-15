package server

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"os"

	"dpl/internal/entities"
)

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
