// Package systemd wraps systemctl commands for service management.
package systemd

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

// serviceDir is the systemd system service directory.
// Override in tests to use a temp directory.
var serviceDir = "/etc/systemd/system"

// commandFn allows overriding exec.CommandContext for testing.
var commandFn = exec.CommandContext

// ServiceName returns the systemd unit name for an app: "dpl-<name>".
func ServiceName(appName string) string {
	return "dpl-" + appName
}

// ServiceFilePath returns the full path to the .service file for an app.
func ServiceFilePath(appName string) string {
	return filepath.Join(serviceDir, ServiceName(appName)+".service")
}

// WriteServiceFile writes the rendered service unit content to the systemd directory.
func WriteServiceFile(appName, content string) error {
	p := ServiceFilePath(appName)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		return fmt.Errorf("systemd write service: %w", err)
	}
	return nil
}

// DaemonReload runs `systemctl daemon-reload`.
func DaemonReload(ctx context.Context) error {
	return run(ctx, "daemon-reload", "systemctl", "daemon-reload")
}

// Enable runs `systemctl enable <unit>`.
func Enable(ctx context.Context, unit string) error {
	return run(ctx, "enable", "systemctl", "enable", unit)
}

// Restart runs `systemctl restart <unit>`.
func Restart(ctx context.Context, unit string) error {
	return run(ctx, "restart", "systemctl", "restart", unit)
}

// run executes a command and wraps any error with the operation label and stderr output.
func run(ctx context.Context, label, name string, args ...string) error {
	cmd := commandFn(ctx, name, args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("systemd %s: %s: %w", label, string(out), err)
	}
	return nil
}
