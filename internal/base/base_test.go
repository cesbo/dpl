package base

import (
	"testing"
)

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

func TestDefaultAddr(t *testing.T) {
	InitEnv()
	if Addr != ":6060" {
		t.Fatal("Addr is empty, expected default :6060")
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

func TestAddrFromEnv(t *testing.T) {
	t.Setenv("DPL_ADDR", "127.0.0.1:9090")
	Addr = ""
	InitEnv()
	if Addr != "127.0.0.1:9090" {
		t.Errorf("Addr = %q, want %q", Addr, "127.0.0.1:9090")
	}
}
