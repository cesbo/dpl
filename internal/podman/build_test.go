package podman

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestSecretPaths(t *testing.T) {
	tests := []struct {
		name       string
		deployDir  string
		layerCount int
		want       []string
	}{
		{
			name:       "single layer",
			deployDir:  "/opt/dpl/myapp/deploy_20260305120000",
			layerCount: 1,
			want:       []string{"/opt/dpl/myapp/deploy_20260305120000/build-sh-1"},
		},
		{
			name:       "two layers",
			deployDir:  "/opt/dpl/myapp/deploy_20260305120000",
			layerCount: 2,
			want: []string{
				"/opt/dpl/myapp/deploy_20260305120000/build-sh-1",
				"/opt/dpl/myapp/deploy_20260305120000/build-sh-2",
			},
		},
		{
			name:       "zero layers",
			deployDir:  "/opt/dpl/myapp/deploy_20260305120000",
			layerCount: 0,
			want:       []string{},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := SecretPaths(tt.deployDir, tt.layerCount)
			if len(got) != len(tt.want) {
				t.Fatalf("len = %d, want %d", len(got), len(tt.want))
			}
			for i := range got {
				if got[i] != tt.want[i] {
					t.Errorf("[%d] = %q, want %q", i, got[i], tt.want[i])
				}
			}
		})
	}
}

func TestImageTag(t *testing.T) {
	tests := []struct {
		name, ts, want string
	}{
		{"myapp", "20260305120000", "localhost/myapp:20260305120000"},
		{"web-api", "20260101000000", "localhost/web-api:20260101000000"},
	}
	for _, tt := range tests {
		if got := ImageTag(tt.name, tt.ts); got != tt.want {
			t.Errorf("ImageTag(%q, %q) = %q, want %q", tt.name, tt.ts, got, tt.want)
		}
	}
}

func TestBuildArgs(t *testing.T) {
	opts := BuildOpts{
		ContextDir: "/tmp/deploy",
		Secrets:    []string{"/tmp/deploy/build-sh-1", "/tmp/deploy/build-sh-2"},
		Tag:        "localhost/myapp:20260305120000",
		LogFile:    "/tmp/deploy/logs/build.log",
	}

	got := buildArgs(opts)
	want := []string{
		"build",
		"--file", "/tmp/deploy/Containerfile",
		"--tag", "localhost/myapp:20260305120000",
		"--secret", "id=build-sh-1,src=/tmp/deploy/build-sh-1",
		"--secret", "id=build-sh-2,src=/tmp/deploy/build-sh-2",
		"/tmp/deploy",
	}

	if len(got) != len(want) {
		t.Fatalf("len = %d, want %d\ngot:  %v\nwant: %v", len(got), len(want), got, want)
	}
	for i := range got {
		if got[i] != want[i] {
			t.Errorf("[%d] = %q, want %q", i, got[i], want[i])
		}
	}
}

func TestBuildArgs_NoSecrets(t *testing.T) {
	opts := BuildOpts{
		ContextDir: "/tmp/deploy",
		Tag:        "localhost/myapp:20260305120000",
	}

	got := buildArgs(opts)
	want := []string{
		"build",
		"--file", "/tmp/deploy/Containerfile",
		"--tag", "localhost/myapp:20260305120000",
		"/tmp/deploy",
	}

	if len(got) != len(want) {
		t.Fatalf("len = %d, want %d\ngot:  %v\nwant: %v", len(got), len(want), got, want)
	}
	for i := range got {
		if got[i] != want[i] {
			t.Errorf("[%d] = %q, want %q", i, got[i], want[i])
		}
	}
}

func TestBuild_WritesLogFile(t *testing.T) {
	dir := t.TempDir()
	logFile := filepath.Join(dir, "build.log")

	// Override commandFn to run "echo hello" instead of podman.
	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		return exec.CommandContext(ctx, "echo", "build output line 1")
	}
	defer func() { commandFn = origCmd }()

	opts := BuildOpts{
		ContextDir: dir,
		Tag:        "localhost/test:123",
		LogFile:    logFile,
	}

	if err := Build(context.Background(), opts); err != nil {
		t.Fatalf("Build: %v", err)
	}

	data, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("read log: %v", err)
	}
	if !strings.Contains(string(data), "build output line 1") {
		t.Errorf("log content = %q, want 'build output line 1'", string(data))
	}
}

func TestBuild_CommandFailure(t *testing.T) {
	dir := t.TempDir()
	logFile := filepath.Join(dir, "build.log")

	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		return exec.CommandContext(ctx, "sh", "-c", "echo 'error output' && exit 1")
	}
	defer func() { commandFn = origCmd }()

	opts := BuildOpts{
		ContextDir: dir,
		Tag:        "localhost/test:123",
		LogFile:    logFile,
	}

	err := Build(context.Background(), opts)
	if err == nil {
		t.Fatal("expected error from failing command")
	}
	if !strings.Contains(err.Error(), "podman build") {
		t.Errorf("error = %q, want 'podman build' prefix", err.Error())
	}

	// Log file should still contain output written before failure.
	data, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("read log: %v", err)
	}
	if !strings.Contains(string(data), "error output") {
		t.Errorf("log content = %q, want 'error output'", string(data))
	}
}

func TestBuild_VerifiesCommand(t *testing.T) {
	dir := t.TempDir()
	logFile := filepath.Join(dir, "build.log")

	// Create fake secret files.
	secret1 := filepath.Join(dir, "build-sh-1")
	os.WriteFile(secret1, []byte("#!/bin/sh\necho layer1"), 0o644)

	var capturedName string
	var capturedArgs []string

	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		capturedName = name
		capturedArgs = args
		return exec.CommandContext(ctx, "true")
	}
	defer func() { commandFn = origCmd }()

	opts := BuildOpts{
		ContextDir: dir,
		Secrets:    []string{secret1},
		Tag:        "localhost/myapp:20260305120000",
		LogFile:    logFile,
	}

	if err := Build(context.Background(), opts); err != nil {
		t.Fatalf("Build: %v", err)
	}

	if capturedName != "podman" {
		t.Errorf("command = %q, want %q", capturedName, "podman")
	}

	// Verify the args contain expected elements.
	argsStr := strings.Join(capturedArgs, " ")
	for _, want := range []string{
		"build",
		"--file",
		"--tag", "localhost/myapp:20260305120000",
		"--secret", "id=build-sh-1",
	} {
		if !strings.Contains(argsStr, want) {
			t.Errorf("args missing %q, got: %v", want, capturedArgs)
		}
	}
}

func TestBuild_InvalidLogPath(t *testing.T) {
	opts := BuildOpts{
		ContextDir: "/tmp",
		Tag:        "localhost/test:123",
		LogFile:    "/nonexistent/dir/build.log",
	}

	err := Build(context.Background(), opts)
	if err == nil {
		t.Fatal("expected error for invalid log path")
	}
	if !strings.Contains(err.Error(), "create log file") {
		t.Errorf("error = %q, want 'create log file'", err.Error())
	}
}
