// Package app implements the app entity: config, templates, and deploy pipeline.
package app

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"gopkg.in/yaml.v3"
)

// Config describes an app entity loaded from config.yaml.
type Config struct {
	Type    string        `yaml:"type"`
	Tokens  []string      `yaml:"tokens"`
	Domain  string        `yaml:"domain"`
	Route   string        `yaml:"route"`
	Image   string        `yaml:"image"`
	Port    int           `yaml:"port"`
	Build   BuildConfig   `yaml:"build"`
	Runtime RuntimeConfig `yaml:"runtime"`
	Volumes []Volume      `yaml:"volumes"`
	Public  PublicConfig  `yaml:"public"`
}

// BuildLayer describes a single build layer with its own COPY and RUN steps.
type BuildLayer struct {
	Name   string            `yaml:"name"`
	Files  []string          `yaml:"files"`
	Env    map[string]string `yaml:"env"`
	Script string            `yaml:"script"`
}

// BuildConfig holds build-time settings as an ordered list of layers.
type BuildConfig struct {
	Layers []BuildLayer `yaml:"layers"`
}

// RuntimeConfig holds runtime settings: environment variables, init script, and start command.
type RuntimeConfig struct {
	Env  map[string]string `yaml:"env"`
	Init string            `yaml:"init"`
	Cmd  string            `yaml:"cmd"`
}

// Volume describes a Podman volume mount.
type Volume struct {
	ID   string `yaml:"id"`
	Path string `yaml:"path"`
}

// PublicConfig holds directories exposed outside the container.
type PublicConfig struct {
	Dirs []PublicDir `yaml:"dirs"`
}

// PublicDir describes a directory inside the container exposed via URL.
type PublicDir struct {
	Path string `yaml:"path"`
	URL  string `yaml:"url"`
}

// ErrUnsupportedType indicates the config.yaml has a type other than "app".
var ErrUnsupportedType = errors.New("unsupported type")

// LoadConfig reads and validates an app config from dir/config.yaml.
func LoadConfig(dir string) (*Config, error) {
	data, err := os.ReadFile(filepath.Join(dir, "config.yaml"))
	if err != nil {
		return nil, fmt.Errorf("load config: %w", err)
	}

	var cfg Config
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("load config: %w", err)
	}

	if cfg.Type != "app" {
		return nil, fmt.Errorf("%w: %s", ErrUnsupportedType, cfg.Type)
	}

	if err := validate(&cfg); err != nil {
		return nil, err
	}

	if cfg.Route == "" {
		cfg.Route = "/"
	}

	return &cfg, nil
}

func validate(cfg *Config) error {
	if len(cfg.Tokens) == 0 {
		return fmt.Errorf("config: tokens are required")
	}
	if cfg.Image == "" {
		return fmt.Errorf("config: image is required")
	}
	if len(cfg.Build.Layers) == 0 {
		return fmt.Errorf("config: build.layers is required")
	}
	for i, l := range cfg.Build.Layers {
		if l.Script == "" {
			return fmt.Errorf("config: build.layers[%d].script is required", i)
		}
	}
	if cfg.Runtime.Cmd == "" {
		return fmt.Errorf("config: runtime.cmd is required")
	}
	if cfg.Port <= 0 {
		return fmt.Errorf("config: port must be positive")
	}
	return nil
}
