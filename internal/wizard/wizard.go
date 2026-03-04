// Package wizard implements the dpl init CLI wizard.
package wizard

import (
	"bufio"
	"embed"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strconv"
	"strings"
	"text/template"
)

//go:embed service.tmpl
var serviceFS embed.FS

const (
	defaultPort        = 6060
	defaultServicePath = "/etc/systemd/system/dpl.service"
)

// ServiceParams holds parameters for the dpl systemd service file.
type ServiceParams struct {
	Port     int
	ExecPath string
}

// Run starts the interactive init wizard that generates
// a systemd service file for dpl itself.
func Run() error {
	execPath, err := os.Executable()
	if err != nil {
		return fmt.Errorf("wizard: detect executable path: %w", err)
	}
	return run(os.Stdin, os.Stdout, execPath)
}

func run(in io.Reader, out io.Writer, execPath string) error {
	slog.Info("init wizard started")
	scanner := bufio.NewScanner(in)

	port, err := promptInt(scanner, out, "HTTP port", defaultPort)
	if err != nil {
		return err
	}
	if port < 1 || port > 65535 {
		return fmt.Errorf("wizard: port %d out of range 1-65535", port)
	}

	servicePath, err := promptString(scanner, out, "Service file path", defaultServicePath)
	if err != nil {
		return err
	}

	params := ServiceParams{
		Port:     port,
		ExecPath: execPath,
	}

	content, err := renderService(params)
	if err != nil {
		return err
	}

	if err := os.WriteFile(servicePath, []byte(content), 0644); err != nil {
		return fmt.Errorf("wizard: write service file: %w", err)
	}

	slog.Info("service file written", "path", servicePath)
	fmt.Fprintf(out, "\nService file written to %s\n", servicePath)
	fmt.Fprintf(out, "Run: systemctl daemon-reload && systemctl enable --now dpl\n")

	return nil
}

func renderService(params ServiceParams) (string, error) {
	tmpl, err := template.ParseFS(serviceFS, "service.tmpl")
	if err != nil {
		return "", fmt.Errorf("wizard: parse template: %w", err)
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, params); err != nil {
		return "", fmt.Errorf("wizard: render template: %w", err)
	}

	return buf.String(), nil
}

func promptInt(scanner *bufio.Scanner, out io.Writer, label string, defaultVal int) (int, error) {
	fmt.Fprintf(out, "%s [%d]: ", label, defaultVal)
	if !scanner.Scan() {
		if err := scanner.Err(); err != nil {
			return 0, fmt.Errorf("wizard: read input: %w", err)
		}
		return defaultVal, nil
	}
	input := strings.TrimSpace(scanner.Text())
	if input == "" {
		return defaultVal, nil
	}
	val, err := strconv.Atoi(input)
	if err != nil {
		return 0, fmt.Errorf("wizard: invalid port %q: %w", input, err)
	}
	return val, nil
}

func promptString(scanner *bufio.Scanner, out io.Writer, label string, defaultVal string) (string, error) {
	fmt.Fprintf(out, "%s [%s]: ", label, defaultVal)
	if !scanner.Scan() {
		if err := scanner.Err(); err != nil {
			return "", fmt.Errorf("wizard: read input: %w", err)
		}
		return defaultVal, nil
	}
	input := strings.TrimSpace(scanner.Text())
	if input == "" {
		return defaultVal, nil
	}
	return input, nil
}
