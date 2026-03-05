package systemd

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestServiceName(t *testing.T) {
	tests := []struct {
		app  string
		want string
	}{
		{"myapp", "dpl-myapp"},
		{"web-api", "dpl-web-api"},
		{"profile", "dpl-profile"},
	}
	for _, tt := range tests {
		if got := ServiceName(tt.app); got != tt.want {
			t.Errorf("ServiceName(%q) = %q, want %q", tt.app, got, tt.want)
		}
	}
}

func TestServiceFilePath(t *testing.T) {
	orig := serviceDir
	serviceDir = "/etc/systemd/system"
	defer func() { serviceDir = orig }()

	tests := []struct {
		app  string
		want string
	}{
		{"myapp", "/etc/systemd/system/dpl-myapp.service"},
		{"web-api", "/etc/systemd/system/dpl-web-api.service"},
	}
	for _, tt := range tests {
		if got := ServiceFilePath(tt.app); got != tt.want {
			t.Errorf("ServiceFilePath(%q) = %q, want %q", tt.app, got, tt.want)
		}
	}
}

func TestWriteServiceFile(t *testing.T) {
	dir := t.TempDir()
	orig := serviceDir
	serviceDir = dir
	defer func() { serviceDir = orig }()

	content := "[Unit]\nDescription=test\n"
	if err := WriteServiceFile("myapp", content); err != nil {
		t.Fatalf("WriteServiceFile: %v", err)
	}

	got, err := os.ReadFile(filepath.Join(dir, "dpl-myapp.service"))
	if err != nil {
		t.Fatalf("read file: %v", err)
	}
	if string(got) != content {
		t.Errorf("content = %q, want %q", string(got), content)
	}
}

func TestWriteServiceFile_InvalidDir(t *testing.T) {
	orig := serviceDir
	serviceDir = "/nonexistent/dir"
	defer func() { serviceDir = orig }()

	err := WriteServiceFile("myapp", "content")
	if err == nil {
		t.Fatal("expected error for invalid directory")
	}
	if !strings.Contains(err.Error(), "systemd write service") {
		t.Errorf("error = %q, want 'systemd write service' prefix", err.Error())
	}
}

func TestDaemonReload(t *testing.T) {
	var capturedName string
	var capturedArgs []string

	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		capturedName = name
		capturedArgs = args
		return exec.CommandContext(ctx, "true")
	}
	defer func() { commandFn = origCmd }()

	if err := DaemonReload(context.Background()); err != nil {
		t.Fatalf("DaemonReload: %v", err)
	}

	if capturedName != "systemctl" {
		t.Errorf("command = %q, want %q", capturedName, "systemctl")
	}
	if len(capturedArgs) != 1 || capturedArgs[0] != "daemon-reload" {
		t.Errorf("args = %v, want [daemon-reload]", capturedArgs)
	}
}

func TestEnable(t *testing.T) {
	var capturedName string
	var capturedArgs []string

	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		capturedName = name
		capturedArgs = args
		return exec.CommandContext(ctx, "true")
	}
	defer func() { commandFn = origCmd }()

	if err := Enable(context.Background(), "dpl-myapp"); err != nil {
		t.Fatalf("Enable: %v", err)
	}

	if capturedName != "systemctl" {
		t.Errorf("command = %q, want %q", capturedName, "systemctl")
	}
	want := []string{"enable", "dpl-myapp"}
	if len(capturedArgs) != len(want) {
		t.Fatalf("args len = %d, want %d", len(capturedArgs), len(want))
	}
	for i := range want {
		if capturedArgs[i] != want[i] {
			t.Errorf("args[%d] = %q, want %q", i, capturedArgs[i], want[i])
		}
	}
}

func TestRestart(t *testing.T) {
	var capturedName string
	var capturedArgs []string

	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		capturedName = name
		capturedArgs = args
		return exec.CommandContext(ctx, "true")
	}
	defer func() { commandFn = origCmd }()

	if err := Restart(context.Background(), "dpl-myapp"); err != nil {
		t.Fatalf("Restart: %v", err)
	}

	if capturedName != "systemctl" {
		t.Errorf("command = %q, want %q", capturedName, "systemctl")
	}
	want := []string{"restart", "dpl-myapp"}
	if len(capturedArgs) != len(want) {
		t.Fatalf("args len = %d, want %d", len(capturedArgs), len(want))
	}
	for i := range want {
		if capturedArgs[i] != want[i] {
			t.Errorf("args[%d] = %q, want %q", i, capturedArgs[i], want[i])
		}
	}
}

func TestDaemonReload_Failure(t *testing.T) {
	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		return exec.CommandContext(ctx, "sh", "-c", "echo 'reload failed' && exit 1")
	}
	defer func() { commandFn = origCmd }()

	err := DaemonReload(context.Background())
	if err == nil {
		t.Fatal("expected error from failing command")
	}
	if !strings.Contains(err.Error(), "systemd daemon-reload") {
		t.Errorf("error = %q, want 'systemd daemon-reload' prefix", err.Error())
	}
	if !strings.Contains(err.Error(), "reload failed") {
		t.Errorf("error = %q, want stderr output 'reload failed'", err.Error())
	}
}

func TestEnable_Failure(t *testing.T) {
	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		return exec.CommandContext(ctx, "sh", "-c", "echo 'enable failed' && exit 1")
	}
	defer func() { commandFn = origCmd }()

	err := Enable(context.Background(), "dpl-myapp")
	if err == nil {
		t.Fatal("expected error")
	}
	if !strings.Contains(err.Error(), "systemd enable") {
		t.Errorf("error = %q, want 'systemd enable' prefix", err.Error())
	}
}

func TestRestart_Failure(t *testing.T) {
	origCmd := commandFn
	commandFn = func(ctx context.Context, name string, args ...string) *exec.Cmd {
		return exec.CommandContext(ctx, "sh", "-c", "echo 'restart failed' && exit 1")
	}
	defer func() { commandFn = origCmd }()

	err := Restart(context.Background(), "dpl-myapp")
	if err == nil {
		t.Fatal("expected error")
	}
	if !strings.Contains(err.Error(), "systemd restart") {
		t.Errorf("error = %q, want 'systemd restart' prefix", err.Error())
	}
}
