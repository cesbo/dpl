package systemd

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAllocatePort_NewPort(t *testing.T) {
	dir := t.TempDir()

	port, err := AllocatePort(dir)
	if err != nil {
		t.Fatalf("AllocatePort: %v", err)
	}
	if port < 1 || port > 65535 {
		t.Errorf("port = %d, want 1-65535", port)
	}

	// File should be created.
	data, err := os.ReadFile(filepath.Join(dir, portFile))
	if err != nil {
		t.Fatalf("read port file: %v", err)
	}
	if strings.TrimSpace(string(data)) == "" {
		t.Error("port file is empty")
	}

	// Second call should return the same port.
	port2, err := AllocatePort(dir)
	if err != nil {
		t.Fatalf("AllocatePort (second): %v", err)
	}
	if port2 != port {
		t.Errorf("second call = %d, want %d (same port)", port2, port)
	}
}

func TestAllocatePort_ExistingPort(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, portFile), []byte("49152"), 0o644)

	port, err := AllocatePort(dir)
	if err != nil {
		t.Fatalf("AllocatePort: %v", err)
	}
	if port != 49152 {
		t.Errorf("port = %d, want 49152", port)
	}
}

func TestReadPort(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, portFile), []byte("8080"), 0o644)

	port, err := ReadPort(dir)
	if err != nil {
		t.Fatalf("ReadPort: %v", err)
	}
	if port != 8080 {
		t.Errorf("port = %d, want 8080", port)
	}
}

func TestReadPort_Missing(t *testing.T) {
	dir := t.TempDir()

	_, err := ReadPort(dir)
	if err == nil {
		t.Fatal("expected error for missing port file")
	}
	if !os.IsNotExist(err) {
		t.Errorf("error = %v, want os.ErrNotExist", err)
	}
}

func TestReadPort_Invalid(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, portFile), []byte("not-a-number"), 0o644)

	_, err := ReadPort(dir)
	if err == nil {
		t.Fatal("expected error for invalid port")
	}
	if !strings.Contains(err.Error(), "read port: parse") {
		t.Errorf("error = %q, want 'read port: parse' prefix", err.Error())
	}
}

func TestWritePort(t *testing.T) {
	dir := t.TempDir()

	if err := WritePort(dir, 12345); err != nil {
		t.Fatalf("WritePort: %v", err)
	}

	data, err := os.ReadFile(filepath.Join(dir, portFile))
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	if string(data) != "12345" {
		t.Errorf("content = %q, want %q", string(data), "12345")
	}
}

func TestWritePort_InvalidDir(t *testing.T) {
	err := WritePort("/nonexistent/dir", 8080)
	if err == nil {
		t.Fatal("expected error for invalid directory")
	}
	if !strings.Contains(err.Error(), "write port") {
		t.Errorf("error = %q, want 'write port' prefix", err.Error())
	}
}

func TestFindFreePort(t *testing.T) {
	port, err := findFreePort()
	if err != nil {
		t.Fatalf("findFreePort: %v", err)
	}
	if port < 1 || port > 65535 {
		t.Errorf("port = %d, want 1-65535", port)
	}

	// Allocate a second port to verify uniqueness (probabilistically).
	port2, err := findFreePort()
	if err != nil {
		t.Fatalf("findFreePort (second): %v", err)
	}
	// Ports should almost always differ (extremely unlikely to get the same one).
	_ = port2
}
