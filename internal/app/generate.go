package app

import (
	"embed"
	"fmt"
	"sort"
	"strings"
	"text/template"
)

//go:embed templates/*.tmpl
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

// BuildScript represents a generated build script for a single layer.
type BuildScript struct {
	Filename string
	Content  string
}

// GenerateBuildScripts renders one build script per layer.
// Scripts are named build-sh-1, build-sh-2, etc. (1-based index).
func GenerateBuildScripts(cfg *Config, uuidFn func() string) ([]BuildScript, error) {
	scripts := make([]BuildScript, len(cfg.Build.Layers))
	for i, layer := range cfg.Build.Layers {
		params := buildShParams{
			Env:    buildEnvEntries(layer.Env, uuidFn),
			Script: layer.Script,
		}
		content, err := renderTemplate("build_sh.tmpl", params)
		if err != nil {
			return nil, fmt.Errorf("layer %d: %w", i, err)
		}
		scripts[i] = BuildScript{
			Filename: fmt.Sprintf("build-sh-%d", i+1),
			Content:  content,
		}
	}
	return scripts, nil
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
		InitScript: cfg.Runtime.Init,
		Cmd:        cfg.Runtime.Cmd,
	}
	return renderTemplate("run_sh.tmpl", params)
}

// containerfileLayer holds data for a single layer in the Containerfile.
type containerfileLayer struct {
	CopyArgs string // e.g. "app/package.json app/yarn.lock ./" or "" if no files
	SecretID string // e.g. "build-sh-1"
}

// containerfileParams holds data for the containerfile.tmpl template.
type containerfileParams struct {
	Image  string
	Layers []containerfileLayer
}

// GenerateContainerfile renders the Containerfile for an app.
// The build context is expected to have app/ subdirectory with the project files.
func GenerateContainerfile(cfg *Config) (string, error) {
	layers := make([]containerfileLayer, len(cfg.Build.Layers))
	for i, l := range cfg.Build.Layers {
		var copyArgs string
		if len(l.Files) > 0 {
			var parts []string
			for _, f := range l.Files {
				if f == "." {
					parts = append(parts, "app/")
				} else {
					parts = append(parts, "app/"+f)
				}
			}
			parts = append(parts, "./")
			copyArgs = strings.Join(parts, " ")
		}
		layers[i] = containerfileLayer{
			CopyArgs: copyArgs,
			SecretID: fmt.Sprintf("build-sh-%d", i+1),
		}
	}
	params := containerfileParams{
		Image:  cfg.Image,
		Layers: layers,
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
	path := "templates/" + name
	tmpl, err := template.ParseFS(templateFS, path)
	if err != nil {
		return "", fmt.Errorf("app: parse template %s: %w", name, err)
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, data); err != nil {
		return "", fmt.Errorf("app: render template %s: %w", name, err)
	}

	return buf.String(), nil
}
