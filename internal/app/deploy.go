package app

import (
	"archive/tar"
	"compress/gzip"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/google/uuid"
)

// maxArchiveSize is the maximum total size of extracted archive content (512 MB).
const maxArchiveSize = 512 << 20

// Deploy orchestrates the app deploy pipeline:
//  1. Creates a deploy directory: baseDir/<name>/deploy_<timestamp>/
//  2. Unpacks the tar.gz archive into deploy dir's app/ subdirectory
//  3. Generates build artifacts (Containerfile, run.sh, build scripts)
//
// On success it returns the deploy directory path.
// On failure any partially created deploy directory is removed.
func Deploy(cfg *Config, name, baseDir string, archive io.Reader) (string, error) {
	ts := time.Now().Format("20060102-150405")
	deployDir := filepath.Join(baseDir, name, "deploy_"+ts)

	if err := os.MkdirAll(deployDir, 0o755); err != nil {
		return "", fmt.Errorf("deploy: create dir: %w", err)
	}

	// Cleanup on any error.
	ok := false
	defer func() {
		if !ok {
			os.RemoveAll(deployDir)
		}
	}()

	// 1. Unpack archive into app/ subdirectory.
	appDir := filepath.Join(deployDir, "app")
	if err := extractArchive(archive, appDir); err != nil {
		return "", fmt.Errorf("deploy: extract archive: %w", err)
	}

	// 2. Generate Containerfile.
	containerfile, err := GenerateContainerfile(cfg)
	if err != nil {
		return "", fmt.Errorf("deploy: %w", err)
	}
	if err := os.WriteFile(filepath.Join(deployDir, "Containerfile"), []byte(containerfile), 0o644); err != nil {
		return "", fmt.Errorf("deploy: write containerfile: %w", err)
	}

	// 3. Generate run.sh.
	runSh, err := GenerateRunSh(cfg, uuid.NewString)
	if err != nil {
		return "", fmt.Errorf("deploy: %w", err)
	}
	if err := os.WriteFile(filepath.Join(deployDir, "run.sh"), []byte(runSh), 0o644); err != nil {
		return "", fmt.Errorf("deploy: write run.sh: %w", err)
	}

	// 4. Generate per-layer build scripts.
	scripts, err := GenerateBuildScripts(cfg, uuid.NewString)
	if err != nil {
		return "", fmt.Errorf("deploy: %w", err)
	}
	for _, s := range scripts {
		if err := os.WriteFile(filepath.Join(deployDir, s.Filename), []byte(s.Content), 0o644); err != nil {
			return "", fmt.Errorf("deploy: write %s: %w", s.Filename, err)
		}
	}

	ok = true
	return deployDir, nil
}

// extractArchive unpacks a tar.gz stream into destDir.
// It rejects entries that escape destDir (absolute paths, ".." components, symlinks).
func extractArchive(r io.Reader, destDir string) error {
	if err := os.MkdirAll(destDir, 0o755); err != nil {
		return fmt.Errorf("create dest dir: %w", err)
	}

	gz, err := gzip.NewReader(r)
	if err != nil {
		return fmt.Errorf("gzip: %w", err)
	}
	defer gz.Close()

	tr := tar.NewReader(gz)
	var totalSize int64

	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return fmt.Errorf("tar: %w", err)
		}

		// Security: reject absolute paths and path traversal.
		clean := filepath.Clean(hdr.Name)
		if filepath.IsAbs(clean) || strings.HasPrefix(clean, "..") {
			return fmt.Errorf("invalid path in archive: %s", hdr.Name)
		}

		target := filepath.Join(destDir, clean)

		// Ensure the resolved path stays under destDir.
		if !strings.HasPrefix(target, filepath.Clean(destDir)+string(filepath.Separator)) &&
			target != filepath.Clean(destDir) {
			return fmt.Errorf("path escapes destination: %s", hdr.Name)
		}

		switch hdr.Typeflag {
		case tar.TypeDir:
			if err := os.MkdirAll(target, 0o755); err != nil {
				return fmt.Errorf("mkdir %s: %w", clean, err)
			}

		case tar.TypeReg:
			totalSize += hdr.Size
			if totalSize > maxArchiveSize {
				return fmt.Errorf("archive exceeds maximum size (%d bytes)", maxArchiveSize)
			}

			if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
				return fmt.Errorf("mkdir for %s: %w", clean, err)
			}

			f, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
			if err != nil {
				return fmt.Errorf("create %s: %w", clean, err)
			}
			if _, err := io.Copy(f, tr); err != nil {
				f.Close()
				return fmt.Errorf("write %s: %w", clean, err)
			}
			f.Close()

		case tar.TypeSymlink, tar.TypeLink:
			return fmt.Errorf("links not allowed in archive: %s", hdr.Name)

		default:
			// Skip other entry types (e.g. pax headers).
		}
	}

	return nil
}
