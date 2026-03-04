package app

import (
	"embed"
	"fmt"
	"sort"
	"strings"
	"text/template"
)

//go:embed *.tmpl
var templateFS embed.FS

// envEntry represents a single environment variable prepared for template rendering.
type envEntry struct {
	Key       string
	Value     string
	Delimiter string
}

// buildEnvEntries converts a map of env vars into a sorted slice of envEntry,
// using uuidFn to generate a unique heredoc delimiter per variable.
func buildEnvEntries(env map[string]string, uuidFn func() string) []envEntry {
	if len(env) == 0 {
		return nil
	}

	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	entries := make([]envEntry, len(keys))
	for i, k := range keys {
		entries[i] = envEntry{
			Key:       k,
			Value:     env[k],
			Delimiter: uuidFn(),
		}
	}
	return entries
}

// buildShParams holds data for the build_sh.tmpl template.
type buildShParams struct {
	Env    []envEntry
	Script string
}

// GenerateBuildSh renders the build.sh script for an app.
func GenerateBuildSh(cfg *Config, uuidFn func() string) (string, error) {
	params := buildShParams{
		Env:    buildEnvEntries(cfg.Build.Env, uuidFn),
		Script: cfg.Build.Script,
	}
	return renderTemplate("build_sh.tmpl", params)
}

// runShParams holds data for the run_sh.tmpl template.
type runShParams struct {
	Env        []envEntry
	InitScript string
	Cmd        string
}

// GenerateRunSh renders the run.sh script for an app.
func GenerateRunSh(cfg *Config, uuidFn func() string) (string, error) {
	params := runShParams{
		Env:        buildEnvEntries(cfg.Runtime.Env, uuidFn),
		InitScript: cfg.Init.Script,
		Cmd:        cfg.Run.Cmd,
	}
	return renderTemplate("run_sh.tmpl", params)
}

// containerfileParams holds data for the containerfile.tmpl template.
type containerfileParams struct {
	Image string
}

// GenerateContainerfile renders the Containerfile for an app.
func GenerateContainerfile(cfg *Config) (string, error) {
	params := containerfileParams{
		Image: cfg.Image,
	}
	return renderTemplate("containerfile.tmpl", params)
}

// ServiceParams holds data for the service.tmpl template.
type ServiceParams struct {
	Volumes       []Volume
	HostPort      int
	ContainerPort int
	ImageRef      string
}

// GenerateService renders the systemd service unit file for an app container.
func GenerateService(cfg *Config, hostPort int, imageRef string) (string, error) {
	params := ServiceParams{
		Volumes:       cfg.Volumes,
		HostPort:      hostPort,
		ContainerPort: cfg.Port,
		ImageRef:      imageRef,
	}
	return renderTemplate("service.tmpl", params)
}

func renderTemplate(name string, data any) (string, error) {
	tmpl, err := template.ParseFS(templateFS, name)
	if err != nil {
		return "", fmt.Errorf("app: parse template %s: %w", name, err)
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, data); err != nil {
		return "", fmt.Errorf("app: render template %s: %w", name, err)
	}

	return buf.String(), nil
}
