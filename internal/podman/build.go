// Package podman wraps podman CLI commands for building and running containers.
package podman

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

// BuildOpts configures a podman build invocation.
type BuildOpts struct {
	// ContextDir is the build context directory (contains Containerfile, app/, run.sh).
	ContextDir string

	// Secrets is a list of absolute paths to build script files (build-sh-1, build-sh-2, ...).
	// Each file is mounted as a secret with id matching its base filename.
	Secrets []string

	// Tag is the full image reference, e.g. "localhost/myapp:20260305120000".
	Tag string

	// LogFile is the path where build output (stdout+stderr) is written.
	LogFile string
}

// Build runs `podman build` with the given options.
// Build output (combined stdout and stderr) is written to opts.LogFile.
func Build(ctx context.Context, opts BuildOpts) error {
	logF, err := os.Create(opts.LogFile)
	if err != nil {
		return fmt.Errorf("podman build: create log file: %w", err)
	}
	defer logF.Close()

	args := buildArgs(opts)
	cmd := newCommand(ctx, "podman", args...)
	cmd.Stdout = logF
	cmd.Stderr = logF

	if err := cmd.Run(); err != nil {
		return fmt.Errorf("podman build: %w", err)
	}
	return nil
}

// buildArgs constructs the argument list for `podman build`.
func buildArgs(opts BuildOpts) []string {
	args := []string{"build"}

	args = append(args, "--file", filepath.Join(opts.ContextDir, "Containerfile"))
	args = append(args, "--tag", opts.Tag)

	for _, s := range opts.Secrets {
		id := filepath.Base(s)
		args = append(args, "--secret", fmt.Sprintf("id=%s,src=%s", id, s))
	}

	args = append(args, opts.ContextDir)
	return args
}

// SecretPaths returns the list of build script paths for a deploy directory.
// Scripts are named build-sh-1, build-sh-2, ..., build-sh-N.
func SecretPaths(deployDir string, layerCount int) []string {
	paths := make([]string, layerCount)
	for i := range layerCount {
		paths[i] = filepath.Join(deployDir, fmt.Sprintf("build-sh-%d", i+1))
	}
	return paths
}

// ImageTag returns the full image reference for an app.
// Format: localhost/<name>:<version>.
func ImageTag(name, version string) string {
	return fmt.Sprintf("localhost/%s:%s", name, version)
}

// commandFn allows overriding exec.CommandContext for testing.
var commandFn = exec.CommandContext

func newCommand(ctx context.Context, name string, args ...string) *exec.Cmd {
	return commandFn(ctx, name, args...)
}
