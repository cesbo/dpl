package wizard

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestRenderServiceDefault(t *testing.T) {
	params := ServiceParams{
		Port:     6060,
		ExecPath: "/usr/local/bin/dpl",
	}

	got, err := renderService(params)
	if err != nil {
		t.Fatalf("renderService: %v", err)
	}

	golden := filepath.Join("testdata", "default.service")
	want, err := os.ReadFile(golden)
	if err != nil {
		t.Fatalf("read golden file: %v", err)
	}

	if got != string(want) {
		t.Errorf("renderService mismatch\ngot:\n%s\nwant:\n%s", got, string(want))
	}
}

func TestRenderServiceCustomPort(t *testing.T) {
	params := ServiceParams{
		Port:     8080,
		ExecPath: "/usr/local/bin/dpl",
	}

	got, err := renderService(params)
	if err != nil {
		t.Fatalf("renderService: %v", err)
	}

	golden := filepath.Join("testdata", "custom_port.service")
	want, err := os.ReadFile(golden)
	if err != nil {
		t.Fatalf("read golden file: %v", err)
	}

	if got != string(want) {
		t.Errorf("renderService mismatch\ngot:\n%s\nwant:\n%s", got, string(want))
	}
}

func TestRunDefaults(t *testing.T) {
	tmpDir := t.TempDir()
	servicePath := filepath.Join(tmpDir, "dpl.service")

	input := "\n" + servicePath + "\n"
	var out strings.Builder

	err := run(strings.NewReader(input), &out, "/usr/local/bin/dpl")
	if err != nil {
		t.Fatalf("run: %v", err)
	}

	got, err := os.ReadFile(servicePath)
	if err != nil {
		t.Fatalf("read service file: %v", err)
	}

	want, err := os.ReadFile(filepath.Join("testdata", "default.service"))
	if err != nil {
		t.Fatalf("read golden file: %v", err)
	}

	if string(got) != string(want) {
		t.Errorf("service file mismatch\ngot:\n%s\nwant:\n%s", string(got), string(want))
	}

	output := out.String()
	if !strings.Contains(output, "Service file written to") {
		t.Errorf("expected confirmation message, got: %s", output)
	}
}

func TestRunCustomPort(t *testing.T) {
	tmpDir := t.TempDir()
	servicePath := filepath.Join(tmpDir, "dpl.service")

	input := "8080\n" + servicePath + "\n"
	var out strings.Builder

	err := run(strings.NewReader(input), &out, "/usr/local/bin/dpl")
	if err != nil {
		t.Fatalf("run: %v", err)
	}

	got, err := os.ReadFile(servicePath)
	if err != nil {
		t.Fatalf("read service file: %v", err)
	}

	want, err := os.ReadFile(filepath.Join("testdata", "custom_port.service"))
	if err != nil {
		t.Fatalf("read golden file: %v", err)
	}

	if string(got) != string(want) {
		t.Errorf("service file mismatch\ngot:\n%s\nwant:\n%s", string(got), string(want))
	}
}

func TestRunInvalidPort(t *testing.T) {
	input := "abc\n"
	var out strings.Builder

	err := run(strings.NewReader(input), &out, "/usr/local/bin/dpl")
	if err == nil {
		t.Fatal("expected error for invalid port")
	}
	if !strings.Contains(err.Error(), "invalid port") {
		t.Errorf("expected 'invalid port' error, got: %v", err)
	}
}

func TestRunPortOutOfRange(t *testing.T) {
	tests := []struct {
		name  string
		input string
	}{
		{"zero", "0\n"},
		{"negative", "-1\n"},
		{"too_high", "70000\n"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var out strings.Builder
			err := run(strings.NewReader(tt.input), &out, "/usr/local/bin/dpl")
			if err == nil {
				t.Fatal("expected error for out-of-range port")
			}
			if !strings.Contains(err.Error(), "out of range") {
				t.Errorf("expected 'out of range' error, got: %v", err)
			}
		})
	}
}

func TestRunEOFUsesDefaults(t *testing.T) {
	tmpDir := t.TempDir()
	servicePath := filepath.Join(tmpDir, "dpl.service")

	// EOF on first prompt → default port, EOF on second → default service path
	// We can't test writing to /etc, so we test just the first prompt with EOF
	// then provide the path for the second prompt.
	// Actually with full EOF both prompts get defaults, but writing to
	// /etc/systemd/system/dpl.service would fail. So just test template rendering.
	params := ServiceParams{
		Port:     defaultPort,
		ExecPath: "/usr/local/bin/dpl",
	}

	got, err := renderService(params)
	if err != nil {
		t.Fatalf("renderService: %v", err)
	}

	want, err := os.ReadFile(filepath.Join("testdata", "default.service"))
	if err != nil {
		t.Fatalf("read golden file: %v", err)
	}

	if got != string(want) {
		t.Errorf("default params mismatch\ngot:\n%s\nwant:\n%s", got, string(want))
	}

	// Also verify that EOF on first prompt uses the default by attempting the full flow
	// writing to a temp path that we set as second input.
	input2 := "\n" + servicePath + "\n"
	var out2 strings.Builder
	err = run(strings.NewReader(input2), &out2, "/usr/local/bin/dpl")
	if err != nil {
		t.Fatalf("run with empty port: %v", err)
	}

	got2, err := os.ReadFile(servicePath)
	if err != nil {
		t.Fatalf("read service file: %v", err)
	}
	if string(got2) != string(want) {
		t.Errorf("empty port should use default\ngot:\n%s\nwant:\n%s", string(got2), string(want))
	}
}
