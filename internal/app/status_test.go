package app

import (
	"os"
	"path/filepath"
	"testing"
)

func TestWriteAndReadStatus(t *testing.T) {
	tests := []struct {
		name      string
		status    string
		errMsg    string
		wantStat  string
		wantErr   string
	}{
		{
			name:     "building",
			status:   StatusBuilding,
			wantStat: StatusBuilding,
		},
		{
			name:     "done",
			status:   StatusDone,
			wantStat: StatusDone,
		},
		{
			name:     "failed with message",
			status:   StatusFailed,
			errMsg:   "exit code 1",
			wantStat: StatusFailed,
			wantErr:  "exit code 1",
		},
		{
			name:     "failed without message",
			status:   StatusFailed,
			wantStat: StatusFailed,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dir := t.TempDir()

			if err := WriteStatus(dir, tt.status, tt.errMsg); err != nil {
				t.Fatalf("WriteStatus: %v", err)
			}

			gotStatus, gotErr, err := ReadStatus(dir)
			if err != nil {
				t.Fatalf("ReadStatus: %v", err)
			}

			if gotStatus != tt.wantStat {
				t.Errorf("status = %q, want %q", gotStatus, tt.wantStat)
			}
			if gotErr != tt.wantErr {
				t.Errorf("errMsg = %q, want %q", gotErr, tt.wantErr)
			}
		})
	}
}

func TestReadStatus_NotFound(t *testing.T) {
	dir := t.TempDir()

	_, _, err := ReadStatus(dir)
	if err == nil {
		t.Fatal("expected error for missing status file")
	}
	if !os.IsNotExist(unwrapAll(err)) {
		t.Errorf("expected not-exist error, got: %v", err)
	}
}

func TestWriteStatus_FileContent(t *testing.T) {
	dir := t.TempDir()

	if err := WriteStatus(dir, StatusFailed, "build error: npm ci failed"); err != nil {
		t.Fatalf("WriteStatus: %v", err)
	}

	data, err := os.ReadFile(filepath.Join(dir, "status"))
	if err != nil {
		t.Fatal(err)
	}

	want := "failed\nbuild error: npm ci failed"
	if string(data) != want {
		t.Errorf("file content = %q, want %q", string(data), want)
	}
}

// unwrapAll fully unwraps an error chain.
func unwrapAll(err error) error {
	for {
		u, ok := err.(interface{ Unwrap() error })
		if !ok {
			return err
		}
		err = u.Unwrap()
	}
}
