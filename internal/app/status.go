package app

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Deploy status constants.
const (
	StatusBuilding = "building"
	StatusDone     = "done"
	StatusFailed   = "failed"
)

// WriteStatus writes the deploy status file into deployDir/status.
// If errMsg is non-empty, it is appended on the next line.
func WriteStatus(deployDir, status, errMsg string) error {
	content := status
	if errMsg != "" {
		content += "\n" + errMsg
	}
	p := filepath.Join(deployDir, "status")
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		return fmt.Errorf("write status: %w", err)
	}
	return nil
}

// ReadStatus reads the deploy status file from deployDir/status.
// Returns the status string and an optional error message.
func ReadStatus(deployDir string) (status, errMsg string, err error) {
	p := filepath.Join(deployDir, "status")
	data, err := os.ReadFile(p)
	if err != nil {
		return "", "", fmt.Errorf("read status: %w", err)
	}
	parts := strings.SplitN(string(data), "\n", 2)
	status = parts[0]
	if len(parts) > 1 {
		errMsg = parts[1]
	}
	return status, errMsg, nil
}
