//go:build integration

package podman

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestIntegration_Build(t *testing.T) {
	// Skip if podman is not available.
	if _, err := exec.LookPath("podman"); err != nil {
		t.Skip("podman not found on PATH, skipping integration test")
	}

	dir := t.TempDir()
	tag := "localhost/dpl-integration-test:" + time.Now().Format("20060102150405")

	// Cleanup: remove the image after test.
	t.Cleanup(func() {
		exec.Command("podman", "rmi", "-f", tag).Run()
	})

	// Write a minimal Containerfile.
	containerfile := "FROM alpine:latest\nRUN --mount=type=secret,id=build-sh-1 /bin/sh /run/secrets/build-sh-1\n"
	if err := os.WriteFile(filepath.Join(dir, "Containerfile"), []byte(containerfile), 0o644); err != nil {
		t.Fatal(err)
	}

	// Write a build script that creates a marker file.
	buildScript := "#!/bin/sh\necho 'integration test marker' > /opt/marker.txt\n"
	secretPath := filepath.Join(dir, "build-sh-1")
	if err := os.WriteFile(secretPath, []byte(buildScript), 0o644); err != nil {
		t.Fatal(err)
	}

	// Create logs directory.
	logsDir := filepath.Join(dir, "logs")
	if err := os.MkdirAll(logsDir, 0o755); err != nil {
		t.Fatal(err)
	}
	logFile := filepath.Join(logsDir, "build.log")

	opts := BuildOpts{
		ContextDir: dir,
		Secrets:    []string{secretPath},
		Tag:        tag,
		LogFile:    logFile,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	if err := Build(ctx, opts); err != nil {
		// Print log file contents for debugging.
		if data, readErr := os.ReadFile(logFile); readErr == nil {
			t.Logf("build log:\n%s", string(data))
		}
		t.Fatalf("Build: %v", err)
	}

	// Verify image exists.
	if err := exec.Command("podman", "image", "exists", tag).Run(); err != nil {
		t.Errorf("image %q does not exist after build", tag)
	}

	// Verify log file was written.
	logData, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("read log file: %v", err)
	}
	if len(logData) == 0 {
		t.Error("log file is empty")
	}

	// Verify the image contains our marker by running a command.
	out, err := exec.Command("podman", "run", "--rm", tag, "cat", "/opt/marker.txt").Output()
	if err != nil {
		t.Fatalf("podman run: %v", err)
	}
	if !strings.Contains(string(out), "integration test marker") {
		t.Errorf("marker file content = %q, want 'integration test marker'", string(out))
	}
}

func TestIntegration_Build_Failure(t *testing.T) {
	if _, err := exec.LookPath("podman"); err != nil {
		t.Skip("podman not found on PATH, skipping integration test")
	}

	dir := t.TempDir()
	tag := "localhost/dpl-integration-fail:" + time.Now().Format("20060102150405")

	t.Cleanup(func() {
		exec.Command("podman", "rmi", "-f", tag).Run()
	})

	// Containerfile that will fail — references a non-existent secret.
	containerfile := "FROM alpine:latest\nRUN false\n"
	os.WriteFile(filepath.Join(dir, "Containerfile"), []byte(containerfile), 0o644)

	logsDir := filepath.Join(dir, "logs")
	os.MkdirAll(logsDir, 0o755)
	logFile := filepath.Join(logsDir, "build.log")

	opts := BuildOpts{
		ContextDir: dir,
		Tag:        tag,
		LogFile:    logFile,
	}

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	err := Build(ctx, opts)
	if err == nil {
		t.Fatal("expected build to fail")
	}

	// Log file should still have content.
	data, readErr := os.ReadFile(logFile)
	if readErr != nil {
		t.Fatalf("read log: %v", readErr)
	}
	if len(data) == 0 {
		t.Error("log file should have output even on failure")
	}
}
