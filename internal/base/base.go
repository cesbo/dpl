// Package base provides global configuration read from environment variables
// and the binary version set at build time via ldflags.
package base

import "os"

// Version is set at build time via:
//
//	-ldflags "-X 'dpl/internal/base.Version=...'"
var Version string = "dev"

// BaseDir is the root directory containing entity subdirectories (default "/opt/dpl").
var BaseDir string

// Port is the HTTP server listen port (default "6060").
var Port string

func InitEnv() {
	BaseDir = os.Getenv("DPL_BASE")
	if BaseDir == "" {
		BaseDir = "/opt/dpl"
	}

	Port = os.Getenv("DPL_PORT")
	if Port == "" {
		Port = "6060"
	}
}
