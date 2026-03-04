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
			Env: map[string]string{
				"NODE_ENV": "production",
				"API_URL":  "https://api.example.com",
			},
			Script: "npm ci\nnpm run build\n",
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
			Script: "npm ci\n",
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

func TestGenerateBuildSh(t *testing.T) {
	got, err := GenerateBuildSh(fullTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateBuildSh: %v", err)
	}
	goldenEqual(t, "build_sh.txt", got)
}

func TestGenerateBuildShMinimal(t *testing.T) {
	got, err := GenerateBuildSh(minimalTestConfig(), testUUIDFn())
	if err != nil {
		t.Fatalf("GenerateBuildSh: %v", err)
	}
	goldenEqual(t, "build_sh_minimal.txt", got)
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
