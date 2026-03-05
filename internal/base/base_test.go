package base

import "testing"

func TestVersionDefault(t *testing.T) {
	if Version != "dev" {
		t.Errorf("Version() = %q, want %q", Version, "dev")
	}
}

func TestDefaultBaseDir(t *testing.T) {
	InitEnv()
	if BaseDir == "" {
		t.Fatal("BaseDir is empty, expected default /opt/dpl")
	}
}

func TestDefaultPort(t *testing.T) {
	InitEnv()
	if Port == "" {
		t.Fatal("Port is empty, expected default 6060")
	}
}

func TestBaseDirFromEnv(t *testing.T) {
	t.Setenv("DPL_BASE", "/tmp/test-dpl")
	// Re-read env (simulate init)
	BaseDir = ""
	InitEnv()
	if BaseDir != "/tmp/test-dpl" {
		t.Errorf("BaseDir = %q, want %q", BaseDir, "/tmp/test-dpl")
	}
}

func TestPortFromEnv(t *testing.T) {
	t.Setenv("DPL_PORT", "9090")
	Port = ""
	InitEnv()
	if Port != "9090" {
		t.Errorf("Port = %q, want %q", Port, "9090")
	}
}
