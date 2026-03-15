// Package server implements the HTTP server and deploy handler.
package server

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"dpl/internal/base"
)

type httpServer struct {
	locker *entityLocker
}

// newMux builds the HTTP routes for the given base directory.
func newMux() *http.ServeMux {
	s := httpServer{
		locker: newEntityLocker(),
	}

	mux := http.NewServeMux()
	mux.Handle("POST /deploy/{name}", authMiddleware(http.HandlerFunc(s.deployHandler)))
	mux.Handle("GET /deploy/{name}/{deployID}/status", authMiddleware(http.HandlerFunc(s.statusHandler)))
	mux.Handle("GET /deploy/{name}/{deployID}/logs", authMiddleware(http.HandlerFunc(s.logsHandler)))

	return mux
}

// entityLocker provides per-entity mutual exclusion for deploy operations.
// It protects version read + check + increment to prevent concurrent deploys.
type entityLocker struct {
	mu    sync.Mutex
	locks map[string]*sync.Mutex
}

func newEntityLocker() *entityLocker {
	return &entityLocker{locks: make(map[string]*sync.Mutex)}
}

// lock returns the mutex for the given entity name, creating it if needed.
func (l *entityLocker) lock(name string) *sync.Mutex {
	l.mu.Lock()
	defer l.mu.Unlock()
	m, ok := l.locks[name]
	if !ok {
		m = &sync.Mutex{}
		l.locks[name] = m
	}
	return m
}

// Run starts the HTTP server and blocks until the context is cancelled.
func Run(ctx context.Context) error {
	srv := &http.Server{
		ReadHeaderTimeout: 5 * time.Second,
		IdleTimeout:       60 * time.Second,
		MaxHeaderBytes:    8 * 1024,
		Addr:              base.Addr,
		Handler:           newMux(),
	}

	errCh := make(chan error, 1)
	go func() {
		slog.Info("server starting", "addr", base.Addr)
		errCh <- srv.ListenAndServe()
	}()

	select {
	case err := <-errCh:
		if err != nil && err != http.ErrServerClosed {
			return fmt.Errorf("server: %w", err)
		}
	case <-ctx.Done():
		slog.Info("shutting down")
		shutCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := srv.Shutdown(shutCtx); err != nil {
			return fmt.Errorf("shutdown: %w", err)
		}
	}

	return nil
}
