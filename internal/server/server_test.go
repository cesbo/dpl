package server

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

// writeConfig creates baseDir/name/config.yaml with the given content.
func writeConfig(t *testing.T, baseDir, name, content string) {
	t.Helper()
	dir := filepath.Join(baseDir, name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestDeploy(t *testing.T) {
	base := t.TempDir()
	writeConfig(t, base, "myapp", `type: app
tokens: ["tok-1", "tok-2"]
`)

	writeConfig(t, base, "unsupported", `type: domain
tokens: ["tok"]
`)

	mux := newMux(base)

	tests := []struct {
		name       string
		method     string
		path       string
		token      string
		wantStatus int
		wantBody   string
	}{
		{
			name:       "success with first token",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantBody:   "deploy started for app \"myapp\"\n",
		},
		{
			name:       "success with second token",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "tok-2",
			wantStatus: http.StatusOK,
			wantBody:   "deploy started for app \"myapp\"\n",
		},
		{
			name:       "entity not found",
			method:     http.MethodPost,
			path:       "/deploy/nonexistent",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantBody:   "entity not found\n",
		},
		{
			name:       "missing auth header",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "",
			wantStatus: http.StatusUnauthorized,
			wantBody:   "missing token\n",
		},
		{
			name:       "invalid token",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "wrong",
			wantStatus: http.StatusUnauthorized,
			wantBody:   "invalid token\n",
		},
		{
			name:       "unsupported entity type",
			method:     http.MethodPost,
			path:       "/deploy/unsupported",
			token:      "tok",
			wantStatus: http.StatusBadRequest,
			wantBody:   "unsupported entity type: domain\n",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(tt.method, tt.path, nil)
			if tt.token != "" {
				req.Header.Set("Authorization", "Bearer "+tt.token)
			}
			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if rec.Code != tt.wantStatus {
				t.Errorf("status = %d, want %d", rec.Code, tt.wantStatus)
			}
			if got := rec.Body.String(); got != tt.wantBody {
				t.Errorf("body = %q, want %q", got, tt.wantBody)
			}
		})
	}
}

func TestDeploy_MethodNotAllowed(t *testing.T) {
	base := t.TempDir()
	writeConfig(t, base, "myapp", `type: app
tokens: ["tok"]
`)
	mux := newMux(base)

	for _, method := range []string{http.MethodGet, http.MethodPut, http.MethodDelete} {
		t.Run(method, func(t *testing.T) {
			req := httptest.NewRequest(method, "/deploy/myapp", nil)
			req.Header.Set("Authorization", "Bearer tok")
			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if rec.Code == http.StatusOK {
				t.Errorf("%s should not return 200", method)
			}
		})
	}
}

func TestExtractBearer(t *testing.T) {
	tests := []struct {
		header string
		want   string
	}{
		{"Bearer abc123", "abc123"},
		{"Bearer ", ""},
		{"bearer abc", ""},
		{"Token abc", ""},
		{"", ""},
	}
	for _, tt := range tests {
		if got := extractBearer(tt.header); got != tt.want {
			t.Errorf("extractBearer(%q) = %q, want %q", tt.header, got, tt.want)
		}
	}
}

func TestMatchToken(t *testing.T) {
	tokens := []string{"alpha", "beta"}

	if !matchToken("alpha", tokens) {
		t.Error("expected alpha to match")
	}
	if !matchToken("beta", tokens) {
		t.Error("expected beta to match")
	}
	if matchToken("gamma", tokens) {
		t.Error("expected gamma not to match")
	}
	if matchToken("", tokens) {
		t.Error("expected empty string not to match")
	}
}

func TestLoadEntityMeta(t *testing.T) {
	base := t.TempDir()
	writeConfig(t, base, "svc", `type: app
tokens: ["a", "b"]
extra: ignored
`)

	meta, err := loadEntityMeta(base, "svc")
	if err != nil {
		t.Fatal(err)
	}
	if meta.Type != "app" {
		t.Errorf("type = %q, want %q", meta.Type, "app")
	}
	if len(meta.Tokens) != 2 || meta.Tokens[0] != "a" || meta.Tokens[1] != "b" {
		t.Errorf("tokens = %v, want [a b]", meta.Tokens)
	}
}

func TestLoadEntityMeta_NotFound(t *testing.T) {
	base := t.TempDir()
	_, err := loadEntityMeta(base, "nope")
	if err == nil {
		t.Fatal("expected error for missing config")
	}
	if !errors.Is(err, os.ErrNotExist) {
		t.Errorf("expected os.ErrNotExist, got: %v", err)
	}
}
