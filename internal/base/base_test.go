package base

import "testing"

func TestVersionDefault(t *testing.T) {
	got := Version()
	if got != "dev" {
		t.Errorf("Version() = %q, want %q", got, "dev")
	}
}

func TestDefaultBaseDir(t *testing.T) {
	if BaseDir == "" {
		t.Fatal("BaseDir is empty, expected default /opt/dpl")
	}
}

func TestDefaultPort(t *testing.T) {
	if Port == "" {
		t.Fatal("Port is empty, expected default 6060")
	}
}

func TestBaseDirFromEnv(t *testing.T) {
	t.Setenv("DPL_BASE", "/tmp/test-dpl")
	// Re-read env (simulate init)
	BaseDir = ""
	init_from_env()
	if BaseDir != "/tmp/test-dpl" {
		t.Errorf("BaseDir = %q, want %q", BaseDir, "/tmp/test-dpl")
	}
}

func TestPortFromEnv(t *testing.T) {
	t.Setenv("DPL_PORT", "9090")
	Port = ""
	init_from_env()
	if Port != "9090" {
		t.Errorf("Port = %q, want %q", Port, "9090")
	}
}
