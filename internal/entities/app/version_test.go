package app

import (
	"os"
	"path/filepath"
	"testing"
)

func TestReadVersion_Missing(t *testing.T) {
	dir := t.TempDir()
	v, err := ReadVersion(dir)
	if err != nil {
		t.Fatalf("ReadVersion: %v", err)
	}
	if v != 0 {
		t.Errorf("version = %d, want 0", v)
	}
}

func TestReadVersion_Exists(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "version.txt"), []byte("42"), 0o644); err != nil {
		t.Fatal(err)
	}
	v, err := ReadVersion(dir)
	if err != nil {
		t.Fatalf("ReadVersion: %v", err)
	}
	if v != 42 {
		t.Errorf("version = %d, want 42", v)
	}
}

func TestReadVersion_WithNewline(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "version.txt"), []byte("7\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	v, err := ReadVersion(dir)
	if err != nil {
		t.Fatalf("ReadVersion: %v", err)
	}
	if v != 7 {
		t.Errorf("version = %d, want 7", v)
	}
}

func TestReadVersion_Invalid(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "version.txt"), []byte("abc"), 0o644); err != nil {
		t.Fatal(err)
	}
	_, err := ReadVersion(dir)
	if err == nil {
		t.Fatal("expected error for invalid version")
	}
}

func TestWriteVersion(t *testing.T) {
	dir := t.TempDir()
	if err := WriteVersion(dir, 5); err != nil {
		t.Fatalf("WriteVersion: %v", err)
	}
	data, err := os.ReadFile(filepath.Join(dir, "version.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "5" {
		t.Errorf("file content = %q, want %q", string(data), "5")
	}
}

func TestWriteVersion_Overwrite(t *testing.T) {
	dir := t.TempDir()
	if err := WriteVersion(dir, 1); err != nil {
		t.Fatal(err)
	}
	if err := WriteVersion(dir, 2); err != nil {
		t.Fatal(err)
	}
	v, err := ReadVersion(dir)
	if err != nil {
		t.Fatal(err)
	}
	if v != 2 {
		t.Errorf("version = %d, want 2", v)
	}
}
