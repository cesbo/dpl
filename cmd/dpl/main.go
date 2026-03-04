package main

import (
	"log/slog"
	"os"

	"dpl/internal/server"
	"dpl/internal/wizard"
)

func main() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, nil)))

	args := os.Args[1:]

	if len(args) == 0 {
		baseDir := os.Getenv("DPL_BASE_DIR")
		if baseDir == "" {
			baseDir = "/opt/dpl"
		}
		if err := server.Run(baseDir); err != nil {
			slog.Error("server failed", "error", err)
			os.Exit(1)
		}
		return
	}

	switch args[0] {
	case "init":
		if err := wizard.Run(); err != nil {
			slog.Error("init failed", "error", err)
			os.Exit(1)
		}
	default:
		slog.Error("unknown command", "command", args[0])
		os.Exit(1)
	}
}
