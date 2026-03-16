package server

import (
	"encoding/json"
	"log/slog"
	"net/http"
)

// errorResponse is the JSON body returned on deploy errors.
type errorResponse struct {
	Error string `json:"error"`
}

// writeJSONError writes a JSON error response with the given status code.
// func writeJSONError(w http.ResponseWriter, code int, msg string) {
// 	w.Header().Set("Content-Type", "application/json")
// 	w.WriteHeader(code)
// 	_ = json.NewEncoder(w).Encode(errorResponse{Error: msg})
// }

// writeError maps an error to an HTTP status code and writes a JSON error response.
func writeError(w http.ResponseWriter, code int, msg string) {
	//
	//
	//
	//
	//
	//
	//
	//
	//
	slog.Error("request failed", "status", code, "error", msg)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(errorResponse{Error: msg})
}
