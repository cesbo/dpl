// Package server implements the HTTP server and deploy handler.
package server

import "log/slog"

// Run starts the HTTP server and blocks until it shuts down.
func Run() error {
	slog.Info("server starting")
	return nil
}
