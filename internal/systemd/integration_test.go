//go:build integration

package systemd

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

func TestIntegration_DaemonReload(t *testing.T) {
	if _, err := exec.LookPath("systemctl"); err != nil {
		t.Skip("systemctl not found on PATH, skipping integration test")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// daemon-reload is safe to call — it just re-reads unit files.
	if err := DaemonReload(ctx); err != nil {
		t.Fatalf("DaemonReload: %v", err)
	}
}

func TestIntegration_EnableRestart(t *testing.T) {
	if _, err := exec.LookPath("systemctl"); err != nil {
		t.Skip("systemctl not found on PATH, skipping integration test")
	}

	// Create a harmless test service.
	unit := "dpl-integration-test"
	serviceContent := `[Unit]
Description=dpl integration test (safe to remove)

[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=no

[Install]
WantedBy=default.target
`

	dir := t.TempDir()
	origDir := serviceDir
	serviceDir = "/etc/systemd/system"
	defer func() { serviceDir = origDir }()

	// Write service file.
	servicePath := filepath.Join("/etc/systemd/system", unit+".service")
	if err := os.WriteFile(servicePath, []byte(serviceContent), 0o644); err != nil {
		t.Fatalf("write service file: %v (may need root)", err)
	}
	_ = dir

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	t.Cleanup(func() {
		exec.Command("systemctl", "disable", unit).Run()
		exec.Command("systemctl", "stop", unit).Run()
		os.Remove(servicePath)
		exec.Command("systemctl", "daemon-reload").Run()
	})

	if err := DaemonReload(ctx); err != nil {
		t.Fatalf("DaemonReload: %v", err)
	}

	if err := Enable(ctx, unit); err != nil {
		t.Fatalf("Enable: %v", err)
	}

	if err := Restart(ctx, unit); err != nil {
		t.Fatalf("Restart: %v", err)
	}
}
