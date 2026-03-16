package server

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"dpl/internal/base"
)

func TestLogsEndpoint(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)
	setupDeployDir(t, "myapp", "deploy_1", "building", "line1\nline2\nline3\n")

	mux := newMux()

	tests := []struct {
		name       string
		path       string
		token      string
		wantStatus int
		wantBody   string
		wantOffset string
		wantErr    string
	}{
		{
			name:       "full log",
			path:       "/deploy/myapp/deploy_1/logs",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantBody:   "line1\nline2\nline3\n",
			wantOffset: "18",
		},
		{
			name:       "log with offset",
			path:       "/deploy/myapp/deploy_1/logs?offset=6",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantBody:   "line2\nline3\n",
			wantOffset: "18",
		},
		{
			name:       "log with offset at end",
			path:       "/deploy/myapp/deploy_1/logs?offset=18",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantBody:   "",
			wantOffset: "18",
		},
		{
			name:       "deploy not found",
			path:       "/deploy/myapp/deploy_999/logs",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantErr:    "log not found",
		},
		{
			name:       "invalid deploy ID",
			path:       "/deploy/myapp/bad-id/logs",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantErr:    "log not found",
		},
		{
			name:       "invalid offset",
			path:       "/deploy/myapp/deploy_1/logs?offset=abc",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
			wantErr:    "invalid offset",
		},
		{
			name:       "negative offset",
			path:       "/deploy/myapp/deploy_1/logs?offset=-1",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
			wantErr:    "invalid offset",
		},
		{
			name:       "auth required",
			path:       "/deploy/myapp/deploy_1/logs",
			token:      "",
			wantStatus: http.StatusUnauthorized,
			wantErr:    "missing token",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodGet, tt.path, nil)
			if tt.token != "" {
				req.Header.Set("Authorization", "Bearer "+tt.token)
			}
			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if tt.wantErr == "" {
				if rec.Code != tt.wantStatus {
					t.Errorf("status = %d, want %d (body: %s)", rec.Code, tt.wantStatus, rec.Body.String())
				}
				if got := rec.Header().Get("Content-Type"); got != "text/plain" {
					t.Errorf("Content-Type = %q, want %q", got, "text/plain")
				}
			}

			if tt.wantBody != "" || (tt.wantStatus == http.StatusOK && tt.wantBody == "") {
				if rec.Body.String() != tt.wantBody {
					t.Errorf("body = %q, want %q", rec.Body.String(), tt.wantBody)
				}
			}

			if tt.wantOffset != "" {
				if got := rec.Header().Get("X-Offset"); got != tt.wantOffset {
					t.Errorf("X-Offset = %q, want %q", got, tt.wantOffset)
				}
			}

			if tt.wantErr != "" {
				assertJSONError(t, rec, tt.wantStatus, tt.wantErr)
			}
		})
	}
}

// TestLogsEndpoint_NoLogFile verifies behavior when deploy dir exists but log file doesn't yet.
func TestLogsEndpoint_NoLogFile(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)
	setupDeployDir(t, "myapp", "deploy_1", "building", "")

	mux := newMux()
	req := httptest.NewRequest(http.MethodGet, "/deploy/myapp/deploy_1/logs", nil)
	req.Header.Set("Authorization", "Bearer tok-1")
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	assertJSONError(t, rec, http.StatusNotFound, "log not found")
}

func TestLogsEndpoint_SuccessRemainsPlainText(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)
	setupDeployDir(t, "myapp", "deploy_1", "building", "line1\n")

	mux := newMux()
	req := httptest.NewRequest(http.MethodGet, "/deploy/myapp/deploy_1/logs", nil)
	req.Header.Set("Authorization", "Bearer tok-1")
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200 (body: %s)", rec.Code, rec.Body.String())
	}
	if got := rec.Header().Get("Content-Type"); got != "text/plain" {
		t.Fatalf("Content-Type = %q, want %q", got, "text/plain")
	}
	if rec.Body.String() != "line1\n" {
		t.Fatalf("body = %q, want %q", rec.Body.String(), "line1\n")
	}
}
