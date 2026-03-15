package server

import (
	"os"
	"path/filepath"
	"testing"

	"dpl/internal/base"
)

// writeConfig creates baseDir/name/config.yaml with the given content.
func writeConfig(t *testing.T, name, content string) {
	t.Helper()
	dir := filepath.Join(base.BaseDir, name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

const validAppConfig = `type: app
tokens: ["tok-1", "tok-2"]
image: node:20-alpine
port: 3000
build:
  - script: npm ci
runtime:
  cmd: "node index.js"
`

// setupDeployDir creates a fake deploy directory with a status file and optionally a build log.
func setupDeployDir(t *testing.T, name, deployID, status, logContent string) {
	t.Helper()
	dir := filepath.Join(base.BaseDir, name, deployID)
	if err := os.MkdirAll(filepath.Join(dir, "logs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "status.txt"), []byte(status), 0o644); err != nil {
		t.Fatal(err)
	}
	if logContent != "" {
		if err := os.WriteFile(filepath.Join(dir, "logs", "build.log"), []byte(logContent), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}
