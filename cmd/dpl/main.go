package main

import (
	"fmt"
	"log/slog"
	"os"

	"dpl/internal/base"
	"dpl/internal/server"
)

func main() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, nil)))

	args := os.Args[1:]

	if len(args) > 0 && (args[0] == "-v" || args[0] == "--version") {
		fmt.Println("dpl", base.Version)
		return
	}

	if len(args) == 0 {
		if err := server.Run(); err != nil {
			slog.Error("server failed", "error", err)
			os.Exit(1)
		}
		return
	}

	slog.Error("unknown command", "command", args[0])
	os.Exit(1)
}
