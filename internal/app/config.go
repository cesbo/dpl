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
	Init    InitConfig    `yaml:"init"`
	Run     RunConfig     `yaml:"run"`
	Volumes []Volume      `yaml:"volumes"`
	Public  PublicConfig  `yaml:"public"`
}

// BuildConfig holds build-time settings.
type BuildConfig struct {
	Env    map[string]string `yaml:"env"`
	Script string            `yaml:"script"`
}

// RuntimeConfig holds runtime environment variables.
type RuntimeConfig struct {
	Env map[string]string `yaml:"env"`
}

// InitConfig holds the pre-start init script.
type InitConfig struct {
	Script string `yaml:"script"`
}

// RunConfig holds the application start command.
type RunConfig struct {
	Cmd string `yaml:"cmd"`
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
	if cfg.Build.Script == "" {
		return fmt.Errorf("config: build.script is required")
	}
	if cfg.Run.Cmd == "" {
		return fmt.Errorf("config: run.cmd is required")
	}
	if cfg.Port <= 0 {
		return fmt.Errorf("config: port must be positive")
	}
	return nil
}
