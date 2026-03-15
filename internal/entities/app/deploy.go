package app

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"

	"dpl/internal/base"
	"dpl/internal/entities"
	"dpl/internal/podman"
	"dpl/internal/systemd"

	"github.com/google/uuid"
)

// maxArchiveSize is the maximum total size of extracted archive content (512 MB).
const maxArchiveSize = 512 << 20

// DeployResult holds the output of a successful Deploy call.
type DeployResult struct {
	Dir     string // Full path to the deploy directory.
	Version int    // Sequential deploy version used in dir name and image tag.
}

// StartDeploy runs the full app deploy pipeline: loads config, manages versioning,
// extracts the archive, generates build artifacts, and kicks off podman build +
// systemd deploy in a background goroutine.
// The caller must provide a per-entity mutex obtained from the entity locker.
// Returns DeployResult for HTTP response or an error mapped to HTTP status codes:
//   - entities.ErrUnsupportedType → 400
//   - os.ErrNotExist → 404
//   - entities.ErrConflict → 409
//   - other errors → 422
func StartDeploy(name string, archive io.Reader, mu *sync.Mutex) (DeployResult, error) {
	entityDir := filepath.Join(base.BaseDir, name)

	cfg, err := LoadConfig(entityDir)
	if err != nil {
		return DeployResult{}, err
	}

	// Acquire per-entity lock for version read + check + increment.
	mu.Lock()

	currentVersion, err := ReadVersion(entityDir)
	if err != nil {
		mu.Unlock()
		return DeployResult{}, fmt.Errorf("start deploy: %w", err)
	}

	// Check if previous deploy is still building.
	if currentVersion > 0 {
		prevDir := filepath.Join(entityDir, "deploy_"+strconv.Itoa(currentVersion))
		status, _, readErr := ReadStatus(prevDir)
		if readErr == nil && status == StatusBuilding {
			mu.Unlock()
			return DeployResult{}, fmt.Errorf("start deploy: %w", entities.ErrConflict)
		}
	}

	newVersion := currentVersion + 1
	if err := WriteVersion(entityDir, newVersion); err != nil {
		mu.Unlock()
		return DeployResult{}, fmt.Errorf("start deploy: %w", err)
	}
	mu.Unlock()

	result, err := Deploy(cfg, name, archive, newVersion)
	if err != nil {
		return DeployResult{}, err
	}

	// Write initial status.
	if err := WriteStatus(result.Dir, StatusBuilding, ""); err != nil {
		return DeployResult{}, fmt.Errorf("start deploy: %w", err)
	}

	// Kick off podman build + systemd deploy in background.
	tag := podman.ImageTag(name, strconv.Itoa(result.Version))
	secrets := podman.SecretPaths(result.Dir, len(cfg.Build))
	logFile := filepath.Join(result.Dir, "logs", "build.log")

	go func() {
		opts := podman.BuildOpts{
			ContextDir: result.Dir,
			Secrets:    secrets,
			Tag:        tag,
			LogFile:    logFile,
		}
		if err := podman.Build(context.Background(), opts); err != nil {
			slog.Error("podman build failed", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}
		slog.Info("podman build completed", "name", name, "tag", tag)

		// Allocate host port and deploy as a systemd service.
		hostPort, err := systemd.AllocatePort(entityDir)
		if err != nil {
			slog.Error("allocate port", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		serviceContent, err := GenerateService(cfg, hostPort, tag)
		if err != nil {
			slog.Error("generate service", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		unit := systemd.ServiceName(name)
		if err := systemd.WriteServiceFile(name, serviceContent); err != nil {
			slog.Error("write service file", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		ctx := context.Background()
		if err := systemd.DaemonReload(ctx); err != nil {
			slog.Error("systemd daemon-reload", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		if err := systemd.Enable(ctx, unit); err != nil {
			slog.Error("systemd enable", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		if err := systemd.Restart(ctx, unit); err != nil {
			slog.Error("systemd restart", "name", name, "error", err)
			WriteStatus(result.Dir, StatusFailed, err.Error())
			return
		}

		slog.Info("deploy completed", "name", name, "unit", unit, "port", hostPort)
		WriteStatus(result.Dir, StatusDone, "")
	}()

	return result, nil
}

// Deploy orchestrates the app deploy pipeline:
//  1. Creates a deploy directory: baseDir/<name>/deploy_<version>/
//  2. Unpacks the tar.gz archive into deploy dir's app/ subdirectory
//  3. Generates build artifacts (Containerfile, run.sh, build scripts)
//  4. Creates logs/ subdirectory for build output
//
// On success it returns a DeployResult with the directory path and version.
// On failure any partially created deploy directory is removed.
func Deploy(cfg *Config, name string, archive io.Reader, version int) (DeployResult, error) {
	deployDir := filepath.Join(base.BaseDir, name, "deploy_"+strconv.Itoa(version))

	if err := os.MkdirAll(deployDir, 0o755); err != nil {
		return DeployResult{}, fmt.Errorf("deploy: create dir: %w", err)
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
		return DeployResult{}, fmt.Errorf("deploy: extract archive: %w", err)
	}

	// 2. Generate Containerfile.
	containerfile, err := GenerateContainerfile(cfg)
	if err != nil {
		return DeployResult{}, fmt.Errorf("deploy: %w", err)
	}
	if err := os.WriteFile(filepath.Join(deployDir, "Containerfile"), []byte(containerfile), 0o644); err != nil {
		return DeployResult{}, fmt.Errorf("deploy: write containerfile: %w", err)
	}

	// 3. Generate run.sh.
	runSh, err := GenerateRunSh(cfg, uuid.NewString)
	if err != nil {
		return DeployResult{}, fmt.Errorf("deploy: %w", err)
	}
	if err := os.WriteFile(filepath.Join(deployDir, "run.sh"), []byte(runSh), 0o644); err != nil {
		return DeployResult{}, fmt.Errorf("deploy: write run.sh: %w", err)
	}

	// 4. Generate per-layer build scripts.
	scripts, err := GenerateBuildScripts(cfg, uuid.NewString)
	if err != nil {
		return DeployResult{}, fmt.Errorf("deploy: %w", err)
	}
	for _, s := range scripts {
		if err := os.WriteFile(filepath.Join(deployDir, s.Filename), []byte(s.Content), 0o644); err != nil {
			return DeployResult{}, fmt.Errorf("deploy: write %s: %w", s.Filename, err)
		}
	}

	// 5. Create logs directory for build output.
	if err := os.MkdirAll(filepath.Join(deployDir, "logs"), 0o755); err != nil {
		return DeployResult{}, fmt.Errorf("deploy: create logs dir: %w", err)
	}

	ok = true
	return DeployResult{Dir: deployDir, Version: version}, nil
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
