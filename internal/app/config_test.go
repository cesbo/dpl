package app

import (
	"errors"
	"os"
	"testing"
)

func TestLoadConfig(t *testing.T) {
	tests := []struct {
		name    string
		dir     string
		wantErr string
	}{
		{
			name: "valid full config",
			dir:  "testdata/valid",
		},
		{
			name: "valid minimal config",
			dir:  "testdata/valid_minimal",
		},
		{
			name:    "unsupported type",
			dir:     "testdata/invalid_type",
			wantErr: "unsupported type: domain",
		},
		{
			name:    "missing tokens",
			dir:     "testdata/missing_tokens",
			wantErr: "config: tokens are required",
		},
		{
			name:    "missing image",
			dir:     "testdata/missing_image",
			wantErr: "config: image is required",
		},
		{
			name:    "missing build script",
			dir:     "testdata/missing_build_script",
			wantErr: "config: build.script is required",
		},
		{
			name:    "missing run cmd",
			dir:     "testdata/missing_run_cmd",
			wantErr: "config: run.cmd is required",
		},
		{
			name:    "missing port",
			dir:     "testdata/missing_port",
			wantErr: "config: port must be positive",
		},
		{
			name:    "invalid yaml",
			dir:     "testdata/invalid_yaml",
			wantErr: "load config:",
		},
		{
			name:    "nonexistent directory",
			dir:     "testdata/nonexistent",
			wantErr: "load config:",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg, err := LoadConfig(tt.dir)

			if tt.wantErr != "" {
				if err == nil {
					t.Fatalf("expected error containing %q, got nil", tt.wantErr)
				}
				if !contains(err.Error(), tt.wantErr) {
					t.Fatalf("expected error containing %q, got %q", tt.wantErr, err.Error())
				}
				return
			}

			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if cfg.Type != "app" {
				t.Errorf("Type = %q, want %q", cfg.Type, "app")
			}
		})
	}
}

func TestLoadConfig_ValidFull(t *testing.T) {
	cfg, err := LoadConfig("testdata/valid")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if got, want := cfg.Domain, "example.com"; got != want {
		t.Errorf("Domain = %q, want %q", got, want)
	}
	if got, want := cfg.Route, "/api"; got != want {
		t.Errorf("Route = %q, want %q", got, want)
	}
	if got, want := cfg.Image, "node:20-alpine"; got != want {
		t.Errorf("Image = %q, want %q", got, want)
	}
	if got, want := cfg.Port, 3000; got != want {
		t.Errorf("Port = %d, want %d", got, want)
	}
	if got, want := len(cfg.Tokens), 2; got != want {
		t.Errorf("len(Tokens) = %d, want %d", got, want)
	}
	if got, want := len(cfg.Build.Env), 2; got != want {
		t.Errorf("len(Build.Env) = %d, want %d", got, want)
	}
	if got, want := len(cfg.Runtime.Env), 2; got != want {
		t.Errorf("len(Runtime.Env) = %d, want %d", got, want)
	}
	if cfg.Init.Script == "" {
		t.Error("Init.Script is empty, want non-empty")
	}
	if got, want := cfg.Run.Cmd, "node server.js"; got != want {
		t.Errorf("Run.Cmd = %q, want %q", got, want)
	}
	if got, want := len(cfg.Volumes), 2; got != want {
		t.Errorf("len(Volumes) = %d, want %d", got, want)
	}
	if got, want := len(cfg.Public.Dirs), 2; got != want {
		t.Errorf("len(Public.Dirs) = %d, want %d", got, want)
	}
}

func TestLoadConfig_DefaultRoute(t *testing.T) {
	cfg, err := LoadConfig("testdata/valid_minimal")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if got, want := cfg.Route, "/"; got != want {
		t.Errorf("Route = %q, want default %q", got, want)
	}
}

func TestLoadConfig_NonexistentDir(t *testing.T) {
	_, err := LoadConfig("testdata/nonexistent")
	if err == nil {
		t.Fatal("expected error for nonexistent dir, got nil")
	}
	if !errors.Is(err, os.ErrNotExist) {
		t.Errorf("expected os.ErrNotExist in chain, got: %v", err)
	}
}

func TestLoadConfig_UnsupportedType(t *testing.T) {
	_, err := LoadConfig("testdata/invalid_type")
	if err == nil {
		t.Fatal("expected error for unsupported type, got nil")
	}
	if !errors.Is(err, ErrUnsupportedType) {
		t.Errorf("expected ErrUnsupportedType in chain, got: %v", err)
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && searchSubstr(s, substr)
}

func searchSubstr(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
