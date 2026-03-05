package systemd

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const portFile = "port.txt"

// AllocatePort returns the persisted host port for the entity, or picks a
// random free port, persists it, and returns it.
func AllocatePort(entityDir string) (int, error) {
	port, err := ReadPort(entityDir)
	if err == nil {
		return port, nil
	}
	if !os.IsNotExist(err) {
		return 0, err
	}

	// Pick a random free port.
	port, err = findFreePort()
	if err != nil {
		return 0, fmt.Errorf("allocate port: %w", err)
	}

	if err := WritePort(entityDir, port); err != nil {
		return 0, err
	}
	return port, nil
}

// ReadPort reads the host port from entityDir/port.txt.
// Returns os.ErrNotExist if the file does not exist.
func ReadPort(entityDir string) (int, error) {
	data, err := os.ReadFile(filepath.Join(entityDir, portFile))
	if err != nil {
		return 0, err
	}
	v, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		return 0, fmt.Errorf("read port: parse %q: %w", string(data), err)
	}
	return v, nil
}

// WritePort writes the host port to entityDir/port.txt.
func WritePort(entityDir string, port int) error {
	p := filepath.Join(entityDir, portFile)
	if err := os.WriteFile(p, []byte(strconv.Itoa(port)), 0o644); err != nil {
		return fmt.Errorf("write port: %w", err)
	}
	return nil
}

// findFreePort binds a TCP listener on 127.0.0.1:0, reads the assigned port,
// and immediately closes the listener.
func findFreePort() (int, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, fmt.Errorf("find free port: %w", err)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	return port, nil
}
