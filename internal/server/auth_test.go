package server

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"dpl/internal/base"
)

func TestExtractBearer(t *testing.T) {
	tests := []struct {
		name   string
		header string
		want   string
	}{
		{"canonical", "Bearer abc123", "abc123"},
		{"lowercase scheme", "bearer abc", "abc"},
		{"uppercase scheme", "BEARER abc", "abc"},
		{"trim surrounding whitespace", "  Bearer abc123  ", "abc123"},
		{"allow repeated separator whitespace", "Bearer   abc123", "abc123"},
		{"allow tab separator", "Bearer\tabc123", "abc123"},
		{"missing token", "Bearer ", ""},
		{"wrong scheme", "Token abc", ""},
		{"missing separator", "Bearerabc", ""},
		{"extra parts", "Bearer abc def", ""},
		{"empty", "", ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := extractBearer(tt.header); got != tt.want {
				t.Errorf("extractBearer(%q) = %q, want %q", tt.header, got, tt.want)
			}
		})
	}
}

func TestAuthMiddleware_AcceptsCaseInsensitiveBearerScheme(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)

	mux := newMux()
	req := httptest.NewRequest(http.MethodGet, "/deploy/myapp/deploy_1/status", nil)
	req.Header.Set("Authorization", "bearer tok-1")
	rec := httptest.NewRecorder()

	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want %d (body: %s)", rec.Code, http.StatusNotFound, rec.Body.String())
	}
}

func TestAuthMiddleware_RejectsMalformedBearerHeader(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)

	mux := newMux()
	req := httptest.NewRequest(http.MethodGet, "/deploy/myapp/deploy_1/status", nil)
	req.Header.Set("Authorization", "Bearer tok-1 extra")
	rec := httptest.NewRecorder()

	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d (body: %s)", rec.Code, http.StatusUnauthorized, rec.Body.String())
	}
	if rec.Body.String() != "missing token\n" {
		t.Fatalf("body = %q, want %q", rec.Body.String(), "missing token\n")
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
	base.BaseDir = t.TempDir()
	writeConfig(t, "svc", `type: app
tokens: ["a", "b"]
extra: ignored
`)

	meta, err := loadEntityMeta("svc")
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
	base.BaseDir = t.TempDir()
	_, err := loadEntityMeta("nope")
	if err == nil {
		t.Fatal("expected error for missing config")
	}
	if !errors.Is(err, os.ErrNotExist) {
		t.Errorf("expected os.ErrNotExist, got: %v", err)
	}
}
