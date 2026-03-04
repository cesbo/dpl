package app

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

// testUUIDFn returns a deterministic UUID generator for golden-file tests.
func testUUIDFn() func() string {
	counter := 0
	return func() string {
		counter++
		return fmt.Sprintf("00000000-0000-0000-0000-%012d", counter)
	}
}

// fullTestConfig returns a Config with all fields populated.
func fullTestConfig() *Config {
	return &Config{
		Type:   "app",
		Tokens: []string{"tok"},
		Image:  "node:20-alpine",
		Port:   3000,
		Build: BuildConfig{
			Layers: []BuildLayer{
				{
					Name:  "deps",
					Files: []string{"package.json", "package-lock.json"},
					Env: map[string]string{
						"NODE_ENV": "production",
					},
					Script: "npm ci\n",
				},
				{
					Name:  "build",
					Files: []string{"."},
					Env: map[string]string{
						"API_URL":  "https://api.example.com",
						"NODE_ENV": "production",
					},
					Script: "npm run build\n",
				},
			},
		},
		Runtime: RuntimeConfig{
			Env: map[string]string{
				"NODE_ENV":     "production",
				"DATABASE_URL": "postgres://localhost/app",
			},
			Init: "npx prisma migrate deploy\n",
			Cmd:  "node server.js",
		},
		Volumes: []Volume{
			{ID: "appdata", Path: "/app/data"},
			{ID: "cache", Path: "/app/.cache"},
		},
	}
}

// minimalTestConfig returns a Config with only required fields.
func minimalTestConfig() *Config {
	return &Config{
		Type:   "app",
		Tokens: []string{"tok"},
		Image:  "node:20-alpine",
		Port:   3000,
		Build: BuildConfig{
			Layers: []BuildLayer{
				{
					Script: "npm ci\n",
				},
			},
		},
		Runtime: RuntimeConfig{
			Cmd: "node index.js",
		},
	}
}

func goldenEqual(t *testing.T, golden string, got string) {
	t.Helper()
	want, err := os.ReadFile(filepath.Join("testdata", golden))
	if err != nil {
		t.Fatalf("read golden file %s: %v", golden, err)
	}
	if got != string(want) {
		t.Errorf("mismatch with %s\ngot:\n%s\nwant:\n%s", golden, got, string(want))
	}
}

func TestGenerateBuildScripts(t *testing.T) {
	scripts, err := GenerateBuildScripts(fullTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateBuildScripts: %v", err)
	}
	if got, want := len(scripts), 2; got != want {
		t.Fatalf("len(scripts) = %d, want %d", got, want)
	}
	if got, want := scripts[0].Filename, "build-sh-1"; got != want {
		t.Errorf("scripts[0].Filename = %q, want %q", got, want)
	}
	if got, want := scripts[1].Filename, "build-sh-2"; got != want {
		t.Errorf("scripts[1].Filename = %q, want %q", got, want)
	}
	goldenEqual(t, "build_sh_layer1.txt", scripts[0].Content)
	goldenEqual(t, "build_sh_layer2.txt", scripts[1].Content)
}

func TestGenerateBuildScriptsMinimal(t *testing.T) {
	scripts, err := GenerateBuildScripts(minimalTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateBuildScripts: %v", err)
	}
	if got, want := len(scripts), 1; got != want {
		t.Fatalf("len(scripts) = %d, want %d", got, want)
	}
	goldenEqual(t, "build_sh_minimal.txt", scripts[0].Content)
}

func TestGenerateRunSh(t *testing.T) {
	got, err := GenerateRunSh(fullTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateRunSh: %v", err)
	}
	goldenEqual(t, "run_sh.txt", got)
}

func TestGenerateRunShMinimal(t *testing.T) {
	got, err := GenerateRunSh(minimalTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateRunSh: %v", err)
	}
	goldenEqual(t, "run_sh_minimal.txt", got)
}

func TestGenerateContainerfile(t *testing.T) {
	got, err := GenerateContainerfile(fullTestConfig())
	if err != nil {
		t.Fatalf("GenerateContainerfile: %v", err)
	}
	goldenEqual(t, "containerfile.txt", got)
}

func TestGenerateContainerfileMinimal(t *testing.T) {
	got, err := GenerateContainerfile(minimalTestConfig())
	if err != nil {
		t.Fatalf("GenerateContainerfile: %v", err)
	}
	goldenEqual(t, "containerfile_minimal.txt", got)
}

func TestGenerateService(t *testing.T) {
	got, err := GenerateService(fullTestConfig(), 49152, "localhost/myapp:20260304120000")
	if err != nil {
		t.Fatalf("GenerateService: %v", err)
	}
	goldenEqual(t, "service.txt", got)
}

func TestGenerateServiceNoVolumes(t *testing.T) {
	got, err := GenerateService(minimalTestConfig(), 49152, "localhost/myapp:20260304120000")
	if err != nil {
		t.Fatalf("GenerateService: %v", err)
	}
	goldenEqual(t, "service_no_volumes.txt", got)
}
