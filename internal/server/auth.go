package server

import (
	"context"
	"crypto/subtle"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"dpl/internal/base"

	"gopkg.in/yaml.v3"
)

// entityMeta holds the minimal fields read from any entity's config.yaml,
// enough for auth validation and type-based dispatch.
type entityMeta struct {
	Type   string   `yaml:"type"`
	Tokens []string `yaml:"tokens"`
}

type contextKey string

const metaKey contextKey = "entityMeta"

// loadEntityMeta reads type and tokens from baseDir/name/config.yaml.
func loadEntityMeta(name string) (*entityMeta, error) {
	p := filepath.Join(base.BaseDir, name, "config.yaml")
	data, err := os.ReadFile(p)
	if err != nil {
		return nil, fmt.Errorf("load entity meta: %w", err)
	}
	var meta entityMeta
	if err := yaml.Unmarshal(data, &meta); err != nil {
		return nil, fmt.Errorf("load entity meta: %w", err)
	}
	return &meta, nil
}

// authMiddleware validates the Bearer token against the entity's config.
func authMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		name := r.PathValue("name")

		meta, err := loadEntityMeta(name)
		if err != nil {
			if errors.Is(err, os.ErrNotExist) {
				http.Error(w, "entity not found", http.StatusNotFound)
				return
			}
			slog.Error("failed to load entity meta", "name", name, "error", err)
			http.Error(w, "internal error", http.StatusInternalServerError)
			return
		}

		token := extractBearer(r.Header.Get("Authorization"))
		if token == "" {
			http.Error(w, "missing token", http.StatusUnauthorized)
			return
		}

		if !matchToken(token, meta.Tokens) {
			http.Error(w, "invalid token", http.StatusUnauthorized)
			return
		}

		ctx := context.WithValue(r.Context(), metaKey, meta)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// extractBearer returns the token from "Bearer <token>", or empty string.
func extractBearer(header string) string {
	fields := strings.Fields(strings.TrimSpace(header))
	if len(fields) != 2 {
		return ""
	}
	if !strings.EqualFold(fields[0], "Bearer") {
		return ""
	}
	return fields[1]
}

// matchToken checks whether tok matches any of the valid tokens using
// constant-time comparison.
func matchToken(tok string, valid []string) bool {
	tokB := []byte(tok)
	for _, v := range valid {
		if subtle.ConstantTimeCompare(tokB, []byte(v)) == 1 {
			return true
		}
	}
	return false
}
