package app

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// createTestArchive builds a tar.gz in memory from filename→content pairs.
func createTestArchive(t *testing.T, files map[string]string) *bytes.Reader {
	t.Helper()

	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)

	for name, content := range files {
		hdr := &tar.Header{
			Name:     name,
			Mode:     0o644,
			Size:     int64(len(content)),
			Typeflag: tar.TypeReg,
		}
		if err := tw.WriteHeader(hdr); err != nil {
			t.Fatalf("write header %s: %v", name, err)
		}
		if _, err := tw.Write([]byte(content)); err != nil {
			t.Fatalf("write body %s: %v", name, err)
		}
	}

	if err := tw.Close(); err != nil {
		t.Fatalf("close tar: %v", err)
	}
	if err := gw.Close(); err != nil {
		t.Fatalf("close gzip: %v", err)
	}

	return bytes.NewReader(buf.Bytes())
}

// createTestArchiveWithDirs builds a tar.gz including explicit directory entries.
func createTestArchiveWithDirs(t *testing.T, dirs []string, files map[string]string) *bytes.Reader {
	t.Helper()

	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)

	for _, dir := range dirs {
		hdr := &tar.Header{
			Name:     dir + "/",
			Mode:     0o755,
			Typeflag: tar.TypeDir,
		}
		if err := tw.WriteHeader(hdr); err != nil {
			t.Fatalf("write dir header %s: %v", dir, err)
		}
	}

	for name, content := range files {
		hdr := &tar.Header{
			Name:     name,
			Mode:     0o644,
			Size:     int64(len(content)),
			Typeflag: tar.TypeReg,
		}
		if err := tw.WriteHeader(hdr); err != nil {
			t.Fatalf("write header %s: %v", name, err)
		}
		if _, err := tw.Write([]byte(content)); err != nil {
			t.Fatalf("write body %s: %v", name, err)
		}
	}

	if err := tw.Close(); err != nil {
		t.Fatalf("close tar: %v", err)
	}
	if err := gw.Close(); err != nil {
		t.Fatalf("close gzip: %v", err)
	}

	return bytes.NewReader(buf.Bytes())
}

// createSymlinkArchive builds a tar.gz containing a symlink entry.
func createSymlinkArchive(t *testing.T, linkName, target string) *bytes.Reader {
	t.Helper()

	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)

	hdr := &tar.Header{
		Name:     linkName,
		Typeflag: tar.TypeSymlink,
		Linkname: target,
	}
	if err := tw.WriteHeader(hdr); err != nil {
		t.Fatalf("write symlink header: %v", err)
	}

	if err := tw.Close(); err != nil {
		t.Fatalf("close tar: %v", err)
	}
	if err := gw.Close(); err != nil {
		t.Fatalf("close gzip: %v", err)
	}

	return bytes.NewReader(buf.Bytes())
}

// createPathTraversalArchive builds a tar.gz with a path traversal entry.
func createPathTraversalArchive(t *testing.T, name string) *bytes.Reader {
	t.Helper()

	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)

	content := "evil"
	hdr := &tar.Header{
		Name:     name,
		Mode:     0o644,
		Size:     int64(len(content)),
		Typeflag: tar.TypeReg,
	}
	if err := tw.WriteHeader(hdr); err != nil {
		t.Fatalf("write header: %v", err)
	}
	if _, err := tw.Write([]byte(content)); err != nil {
		t.Fatalf("write body: %v", err)
	}

	if err := tw.Close(); err != nil {
		t.Fatalf("close tar: %v", err)
	}
	if err := gw.Close(); err != nil {
		t.Fatalf("close gzip: %v", err)
	}

	return bytes.NewReader(buf.Bytes())
}

func minimalConfig() *Config {
	return &Config{
		Type:   "app",
		Tokens: []string{"secret"},
		Image:  "node:20-alpine",
		Port:   3000,
		Build: []BuildLayer{
			{Script: "npm ci"},
		},
		Runtime: RuntimeConfig{
			Cmd: "node index.js",
		},
	}
}

func TestDeploy_HappyPath(t *testing.T) {
	base := t.TempDir()
	name := "myapp"
	cfg := minimalConfig()

	// Create the entity directory (required by the path).
	if err := os.MkdirAll(filepath.Join(base, name), 0o755); err != nil {
		t.Fatal(err)
	}

	archive := createTestArchive(t, map[string]string{
		"index.js":       "console.log('hello')",
		"package.json":   `{"name":"myapp"}`,
		"src/main.js":    "// main",
	})

	result, err := Deploy(cfg, name, base, archive, 1)
	if err != nil {
		t.Fatalf("Deploy: %v", err)
	}

	deployDir := result.Dir

	// Verify version.
	if result.Version != 1 {
		t.Errorf("version = %d, want 1", result.Version)
	}

	// Verify deploy dir exists and is under baseDir/name/.
	if !strings.HasPrefix(deployDir, filepath.Join(base, name, "deploy_")) {
		t.Errorf("deployDir = %q, expected prefix %q", deployDir, filepath.Join(base, name, "deploy_"))
	}

	// Verify app/ subdirectory with unpacked files.
	for _, f := range []string{"index.js", "package.json", "src/main.js"} {
		path := filepath.Join(deployDir, "app", f)
		if _, err := os.Stat(path); err != nil {
			t.Errorf("expected file %s to exist: %v", f, err)
		}
	}

	// Verify content of unpacked file.
	data, err := os.ReadFile(filepath.Join(deployDir, "app", "index.js"))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "console.log('hello')" {
		t.Errorf("index.js content = %q", string(data))
	}

	// Verify generated artifacts exist.
	for _, f := range []string{"Containerfile", "run.sh", "build-sh-1"} {
		path := filepath.Join(deployDir, f)
		if _, err := os.Stat(path); err != nil {
			t.Errorf("expected artifact %s to exist: %v", f, err)
		}
	}

	// Verify Containerfile contains expected image.
	cf, err := os.ReadFile(filepath.Join(deployDir, "Containerfile"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(cf), "FROM node:20-alpine") {
		t.Errorf("Containerfile missing FROM line, got:\n%s", string(cf))
	}

	// Verify run.sh contains the cmd.
	rs, err := os.ReadFile(filepath.Join(deployDir, "run.sh"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(rs), "exec node index.js") {
		t.Errorf("run.sh missing exec cmd, got:\n%s", string(rs))
	}

	// Verify build script contains the script.
	bs, err := os.ReadFile(filepath.Join(deployDir, "build-sh-1"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(bs), "npm ci") {
		t.Errorf("build-sh-1 missing script, got:\n%s", string(bs))
	}

	// Verify logs/ directory exists.
	if fi, err := os.Stat(filepath.Join(deployDir, "logs")); err != nil {
		t.Errorf("expected logs/ dir to exist: %v", err)
	} else if !fi.IsDir() {
		t.Errorf("logs/ should be a directory")
	}
}

func TestDeploy_MultipleLayersBuildScripts(t *testing.T) {
	base := t.TempDir()
	name := "multi"
	cfg := &Config{
		Type:   "app",
		Tokens: []string{"tok"},
		Image:  "node:20-alpine",
		Port:   3000,
		Build: []BuildLayer{
			{Files: []string{"package.json"}, Script: "npm ci"},
			{Files: []string{"."}, Script: "npm run build"},
		},
		Runtime: RuntimeConfig{
			Cmd: "node server.js",
		},
	}

	if err := os.MkdirAll(filepath.Join(base, name), 0o755); err != nil {
		t.Fatal(err)
	}

	archive := createTestArchive(t, map[string]string{
		"package.json": "{}",
		"server.js":    "// server",
	})

	result, err := Deploy(cfg, name, base, archive, 1)
	if err != nil {
		t.Fatalf("Deploy: %v", err)
	}

	deployDir := result.Dir

	// Two build scripts should exist.
	for _, f := range []string{"build-sh-1", "build-sh-2"} {
		if _, err := os.Stat(filepath.Join(deployDir, f)); err != nil {
			t.Errorf("expected %s to exist: %v", f, err)
		}
	}

	bs2, err := os.ReadFile(filepath.Join(deployDir, "build-sh-2"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(bs2), "npm run build") {
		t.Errorf("build-sh-2 missing script")
	}
}

func TestDeploy_DirectoryStructureInArchive(t *testing.T) {
	base := t.TempDir()
	name := "dirapp"
	cfg := minimalConfig()

	if err := os.MkdirAll(filepath.Join(base, name), 0o755); err != nil {
		t.Fatal(err)
	}

	archive := createTestArchiveWithDirs(t, []string{"src", "src/lib"}, map[string]string{
		"src/index.js":     "// index",
		"src/lib/utils.js": "// utils",
	})

	result, err := Deploy(cfg, name, base, archive, 1)
	if err != nil {
		t.Fatalf("Deploy: %v", err)
	}

	deployDir := result.Dir

	for _, f := range []string{"src/index.js", "src/lib/utils.js"} {
		if _, err := os.Stat(filepath.Join(deployDir, "app", f)); err != nil {
			t.Errorf("expected %s in app/: %v", f, err)
		}
	}
}

func TestExtractArchive_PathTraversal(t *testing.T) {
	tests := []struct {
		name string
		path string
	}{
		{"dotdot prefix", "../etc/passwd"},
		{"nested dotdot", "foo/../../etc/passwd"},
		{"absolute path", "/etc/passwd"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dest := t.TempDir()
			archive := createPathTraversalArchive(t, tt.path)
			err := extractArchive(archive, dest)
			if err == nil {
				t.Error("expected error for path traversal")
			}
		})
	}
}

func TestExtractArchive_RejectsSymlinks(t *testing.T) {
	dest := t.TempDir()
	archive := createSymlinkArchive(t, "link", "/etc/passwd")
	err := extractArchive(archive, dest)
	if err == nil {
		t.Error("expected error for symlink")
	}
	if !strings.Contains(err.Error(), "links not allowed") {
		t.Errorf("unexpected error: %v", err)
	}
}

func TestExtractArchive_InvalidGzip(t *testing.T) {
	dest := t.TempDir()
	err := extractArchive(strings.NewReader("not gzip data"), dest)
	if err == nil {
		t.Error("expected error for invalid gzip")
	}
}

func TestDeploy_CleanupOnFailure(t *testing.T) {
	base := t.TempDir()
	name := "failapp"
	cfg := minimalConfig()

	if err := os.MkdirAll(filepath.Join(base, name), 0o755); err != nil {
		t.Fatal(err)
	}

	// Pass invalid data (not gzip) to trigger an error during extraction.
	_, err := Deploy(cfg, name, base, strings.NewReader("invalid archive"), 1)
	if err == nil {
		t.Fatal("expected error")
	}

	// Verify no deploy_* directories remain.
	entries, err := os.ReadDir(filepath.Join(base, name))
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if strings.HasPrefix(e.Name(), "deploy_") {
			t.Errorf("deploy dir %s should have been cleaned up", e.Name())
		}
	}
}

func TestDeploy_ArchiveContents(t *testing.T) {
	base := t.TempDir()
	name := "contentapp"
	cfg := minimalConfig()

	if err := os.MkdirAll(filepath.Join(base, name), 0o755); err != nil {
		t.Fatal(err)
	}

	files := map[string]string{
		"index.js":    "const x = 1;",
		"lib/util.js": "module.exports = {};",
		"README.md":   "# Hello",
	}
	archive := createTestArchive(t, files)

	result, err := Deploy(cfg, name, base, archive, 1)
	if err != nil {
		t.Fatalf("Deploy: %v", err)
	}

	deployDir := result.Dir

	// Verify every file has correct content.
	for name, want := range files {
		got, err := os.ReadFile(filepath.Join(deployDir, "app", name))
		if err != nil {
			t.Errorf("read %s: %v", name, err)
			continue
		}
		if string(got) != want {
			t.Errorf("%s content = %q, want %q", name, string(got), want)
		}
	}
}

func TestExtractArchive_EmptyArchive(t *testing.T) {
	dest := t.TempDir()

	// Create an empty tar.gz.
	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)
	tw.Close()
	gw.Close()

	err := extractArchive(bytes.NewReader(buf.Bytes()), dest)
	if err != nil {
		t.Errorf("expected no error for empty archive, got: %v", err)
	}
}

// BenchmarkExtractArchive measures extraction performance.
func BenchmarkExtractArchive(b *testing.B) {
	// Build a test archive with several files.
	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)
	content := strings.Repeat("x", 1024)
	for i := 0; i < 100; i++ {
		name := "file" + strings.Repeat("x", 3) + ".txt"
		hdr := &tar.Header{
			Name:     name,
			Mode:     0o644,
			Size:     int64(len(content)),
			Typeflag: tar.TypeReg,
		}
		tw.WriteHeader(hdr)
		io.WriteString(tw, content)
	}
	tw.Close()
	gw.Close()
	archiveData := buf.Bytes()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		dest := b.TempDir()
		extractArchive(bytes.NewReader(archiveData), dest)
	}
}
