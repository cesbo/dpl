package app

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const versionFile = "version.txt"

// ReadVersion reads the current deploy version from entityDir/version.txt.
// Returns 0 if the file does not exist (no deploys yet).
func ReadVersion(entityDir string) (int, error) {
	data, err := os.ReadFile(filepath.Join(entityDir, versionFile))
	if err != nil {
		if os.IsNotExist(err) {
			return 0, nil
		}
		return 0, fmt.Errorf("read version: %w", err)
	}
	v, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		return 0, fmt.Errorf("read version: parse %q: %w", string(data), err)
	}
	return v, nil
}

// WriteVersion writes the deploy version to entityDir/version.txt.
func WriteVersion(entityDir string, version int) error {
	p := filepath.Join(entityDir, versionFile)
	if err := os.WriteFile(p, []byte(strconv.Itoa(version)), 0o644); err != nil {
		return fmt.Errorf("write version: %w", err)
	}
	return nil
}
