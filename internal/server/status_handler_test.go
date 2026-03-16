package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"dpl/internal/base"
)

func TestStatusEndpoint(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)
	setupDeployDir(t, "myapp", "deploy_1", "building", "")
	setupDeployDir(t, "myapp", "deploy_2", "done", "")
	setupDeployDir(t, "myapp", "deploy_3", "failed\nexit code 1", "")

	mux := newMux()

	tests := []struct {
		name       string
		path       string
		token      string
		wantStatus int
		wantJSON   *statusResponse
		wantErr    string
	}{
		{
			name:       "building status",
			path:       "/deploy/myapp/deploy_1/status",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantJSON:   &statusResponse{DeployID: "deploy_1", Status: "building"},
		},
		{
			name:       "done status",
			path:       "/deploy/myapp/deploy_2/status",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantJSON:   &statusResponse{DeployID: "deploy_2", Status: "done"},
		},
		{
			name:       "failed status with error",
			path:       "/deploy/myapp/deploy_3/status",
			token:      "tok-1",
			wantStatus: http.StatusOK,
			wantJSON:   &statusResponse{DeployID: "deploy_3", Status: "failed", Error: "exit code 1"},
		},
		{
			name:       "deploy not found",
			path:       "/deploy/myapp/deploy_999/status",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantErr:    "deploy not found",
		},
		{
			name:       "invalid deploy ID format",
			path:       "/deploy/myapp/bad-id/status",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantErr:    "deploy not found",
		},
		{
			name:       "auth required",
			path:       "/deploy/myapp/deploy_1/status",
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

			if tt.wantJSON != nil {
				if rec.Code != tt.wantStatus {
					t.Errorf("status = %d, want %d (body: %s)", rec.Code, tt.wantStatus, rec.Body.String())
				}
				if got := rec.Header().Get("Content-Type"); got != "application/json" {
					t.Fatalf("Content-Type = %q, want %q", got, "application/json")
				}

				var got statusResponse
				if err := json.NewDecoder(rec.Body).Decode(&got); err != nil {
					t.Fatalf("decode: %v (body: %s)", err, rec.Body.String())
				}
				if got.DeployID != tt.wantJSON.DeployID {
					t.Errorf("deploy_id = %q, want %q", got.DeployID, tt.wantJSON.DeployID)
				}
				if got.Status != tt.wantJSON.Status {
					t.Errorf("status = %q, want %q", got.Status, tt.wantJSON.Status)
				}
				if got.Error != tt.wantJSON.Error {
					t.Errorf("error = %q, want %q", got.Error, tt.wantJSON.Error)
				}
			}

			if tt.wantErr != "" {
				assertJSONError(t, rec, tt.wantStatus, tt.wantErr)
			}
		})
	}
}
