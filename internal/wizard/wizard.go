// Package wizard implements the dpl init CLI wizard.
package wizard

import "log/slog"

// Run starts the interactive init wizard that generates
// a systemd service file for dpl itself.
func Run() error {
	slog.Info("init wizard started")
	return nil
}
