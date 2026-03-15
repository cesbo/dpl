package server

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"dpl/internal/base"
)

// testArchiveBody returns a minimal tar.gz body for deploy testing.
func testArchiveBody(t *testing.T) *bytes.Reader {
	t.Helper()

	var buf bytes.Buffer
	gw := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gw)

	content := "console.log('hello')"
	hdr := &tar.Header{
		Name:     "index.js",
		Mode:     0o644,
		Size:     int64(len(content)),
		Typeflag: tar.TypeReg,
	}
	if err := tw.WriteHeader(hdr); err != nil {
		t.Fatal(err)
	}
	if _, err := tw.Write([]byte(content)); err != nil {
		t.Fatal(err)
	}
	if err := tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gw.Close(); err != nil {
		t.Fatal(err)
	}

	return bytes.NewReader(buf.Bytes())
}

func TestDeploy(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)
	writeConfig(t, "myapp2", validAppConfig)

	writeConfig(t, "unsupported", `type: domain
tokens: ["tok"]
`)

	mux := newMux()

	tests := []struct {
		name       string
		method     string
		path       string
		token      string
		body       func() *bytes.Reader
		wantStatus int
		wantBody   string
	}{
		{
			name:       "successful deploy",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "tok-1",
			body:       func() *bytes.Reader { return testArchiveBody(t) },
			wantStatus: http.StatusAccepted,
			wantBody:   `"deploy_id":"deploy_`,
		},
		{
			name:       "success with second token",
			method:     http.MethodPost,
			path:       "/deploy/myapp2",
			token:      "tok-2",
			body:       func() *bytes.Reader { return testArchiveBody(t) },
			wantStatus: http.StatusAccepted,
			wantBody:   `"deploy_id":"deploy_`,
		},
		{
			name:       "entity not found",
			method:     http.MethodPost,
			path:       "/deploy/nonexistent",
			token:      "tok-1",
			wantStatus: http.StatusNotFound,
			wantBody:   "entity not found\n",
		},
		{
			name:       "missing auth header",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "",
			wantStatus: http.StatusUnauthorized,
			wantBody:   "missing token\n",
		},
		{
			name:       "invalid token",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "wrong",
			wantStatus: http.StatusUnauthorized,
			wantBody:   "invalid token\n",
		},
		{
			name:       "unsupported entity type",
			method:     http.MethodPost,
			path:       "/deploy/unsupported",
			token:      "tok",
			wantStatus: http.StatusBadRequest,
			wantBody:   "unsupported entity type: domain\n",
		},
		{
			name:       "invalid archive body",
			method:     http.MethodPost,
			path:       "/deploy/myapp",
			token:      "tok-1",
			body:       func() *bytes.Reader { return bytes.NewReader([]byte("not gzip")) },
			wantStatus: http.StatusUnprocessableEntity,
			wantBody:   "extract archive",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var body *bytes.Reader
			if tt.body != nil {
				body = tt.body()
			}

			var req *http.Request
			if body != nil {
				req = httptest.NewRequest(tt.method, tt.path, body)
			} else {
				req = httptest.NewRequest(tt.method, tt.path, nil)
			}
			if tt.token != "" {
				req.Header.Set("Authorization", "Bearer "+tt.token)
			}

			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if rec.Code != tt.wantStatus {
				t.Errorf("status = %d, want %d (body: %s)", rec.Code, tt.wantStatus, rec.Body.String())
			}
			if tt.wantBody != "" {
				if got := rec.Body.String(); !strings.Contains(got, tt.wantBody) {
					t.Errorf("body = %q, want substring %q", got, tt.wantBody)
				}
			}
		})
	}
}

// TestDeploy_CreatesFiles verifies that a successful deploy creates the expected
// files in the deploy directory.
func TestDeploy_CreatesFiles(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)

	mux := newMux()

	req := httptest.NewRequest(http.MethodPost, "/deploy/myapp", testArchiveBody(t))
	req.Header.Set("Authorization", "Bearer tok-1")
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusAccepted {
		t.Fatalf("status = %d, want 202 (body: %s)", rec.Code, rec.Body.String())
	}

	var resp deployResponse
	if err := json.NewDecoder(rec.Body).Decode(&resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if !strings.HasPrefix(resp.DeployID, "deploy_") {
		t.Fatalf("deploy_id = %q, want prefix 'deploy_'", resp.DeployID)
	}

	deployDir := filepath.Join(base.BaseDir, "myapp", resp.DeployID)

	for _, fileName := range []string{"Containerfile", "run.sh", "build-sh-1", "app/index.js", "logs"} {
		if _, err := os.Stat(filepath.Join(deployDir, fileName)); err != nil {
			t.Errorf("expected %s to exist: %v", fileName, err)
		}
	}

	data, err := os.ReadFile(filepath.Join(deployDir, "status.txt"))
	if err != nil {
		t.Fatalf("read status file: %v", err)
	}
	if string(data) != "building" {
		t.Errorf("status = %q, want %q", string(data), "building")
	}
}

func TestDeploy_MethodNotAllowed(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", `type: app
tokens: ["tok"]
`)
	mux := newMux()

	for _, method := range []string{http.MethodGet, http.MethodPut, http.MethodDelete} {
		t.Run(method, func(t *testing.T) {
			req := httptest.NewRequest(method, "/deploy/myapp", nil)
			req.Header.Set("Authorization", "Bearer tok")
			rec := httptest.NewRecorder()
			mux.ServeHTTP(rec, req)

			if rec.Code == http.StatusOK {
				t.Errorf("%s should not return 200", method)
			}
		})
	}
}

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
		wantBody   string
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
			wantBody:   "deploy not found",
		},
		{
			name:       "invalid deploy ID format",
			path:       "/deploy/myapp/bad-id/status",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
			wantBody:   "invalid deploy ID",
		},
		{
			name:       "deploy ID too short",
			path:       "/deploy/myapp/deploy_/status",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
			wantBody:   "invalid deploy ID",
		},
		{
			name:       "auth required",
			path:       "/deploy/myapp/deploy_1/status",
			token:      "",
			wantStatus: http.StatusUnauthorized,
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

			if rec.Code != tt.wantStatus {
				t.Errorf("status = %d, want %d (body: %s)", rec.Code, tt.wantStatus, rec.Body.String())
			}

			if tt.wantJSON != nil {
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

			if tt.wantBody != "" && !strings.Contains(rec.Body.String(), tt.wantBody) {
				t.Errorf("body = %q, want substring %q", rec.Body.String(), tt.wantBody)
			}
		})
	}
}

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
		},
		{
			name:       "invalid deploy ID",
			path:       "/deploy/myapp/bad-id/logs",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
		},
		{
			name:       "invalid offset",
			path:       "/deploy/myapp/deploy_1/logs?offset=abc",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
		},
		{
			name:       "negative offset",
			path:       "/deploy/myapp/deploy_1/logs?offset=-1",
			token:      "tok-1",
			wantStatus: http.StatusBadRequest,
		},
		{
			name:       "auth required",
			path:       "/deploy/myapp/deploy_1/logs",
			token:      "",
			wantStatus: http.StatusUnauthorized,
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

			if rec.Code != tt.wantStatus {
				t.Errorf("status = %d, want %d (body: %s)", rec.Code, tt.wantStatus, rec.Body.String())
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

	if rec.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404 (body: %s)", rec.Code, rec.Body.String())
	}
}

// TestDeploy_Conflict409 verifies that a second deploy returns 409
// when the previous deploy is still in "building" status.
func TestDeploy_Conflict409(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)

	entityDir := filepath.Join(base.BaseDir, "myapp")
	if err := os.WriteFile(filepath.Join(entityDir, "version.txt"), []byte("1"), 0o644); err != nil {
		t.Fatal(err)
	}
	setupDeployDir(t, "myapp", "deploy_1", "building", "")

	mux := newMux()

	req := httptest.NewRequest(http.MethodPost, "/deploy/myapp", testArchiveBody(t))
	req.Header.Set("Authorization", "Bearer tok-1")
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, req)

	if rec.Code != http.StatusConflict {
		t.Errorf("status = %d, want 409 (body: %s)", rec.Code, rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), "deploy already in progress") {
		t.Errorf("body = %q, want substring %q", rec.Body.String(), "deploy already in progress")
	}
}

// TestDeploy_SequentialVersions verifies that sequential deploys get
// incrementing version numbers and version.txt is updated.
func TestDeploy_SequentialVersions(t *testing.T) {
	base.BaseDir = t.TempDir()
	writeConfig(t, "myapp", validAppConfig)

	mux := newMux()

	req1 := httptest.NewRequest(http.MethodPost, "/deploy/myapp", testArchiveBody(t))
	req1.Header.Set("Authorization", "Bearer tok-1")
	rec1 := httptest.NewRecorder()
	mux.ServeHTTP(rec1, req1)

	if rec1.Code != http.StatusAccepted {
		t.Fatalf("deploy 1: status = %d, want 202", rec1.Code)
	}

	var resp1 deployResponse
	if err := json.NewDecoder(rec1.Body).Decode(&resp1); err != nil {
		t.Fatal(err)
	}
	if resp1.DeployID != "deploy_1" {
		t.Errorf("deploy 1: id = %q, want %q", resp1.DeployID, "deploy_1")
	}

	deployDir1 := filepath.Join(base.BaseDir, "myapp", "deploy_1")
	if err := os.WriteFile(filepath.Join(deployDir1, "status.txt"), []byte("done"), 0o644); err != nil {
		t.Fatal(err)
	}

	req2 := httptest.NewRequest(http.MethodPost, "/deploy/myapp", testArchiveBody(t))
	req2.Header.Set("Authorization", "Bearer tok-1")
	rec2 := httptest.NewRecorder()
	mux.ServeHTTP(rec2, req2)

	if rec2.Code != http.StatusAccepted {
		t.Fatalf("deploy 2: status = %d, want 202", rec2.Code)
	}

	var resp2 deployResponse
	if err := json.NewDecoder(rec2.Body).Decode(&resp2); err != nil {
		t.Fatal(err)
	}
	if resp2.DeployID != "deploy_2" {
		t.Errorf("deploy 2: id = %q, want %q", resp2.DeployID, "deploy_2")
	}

	data, err := os.ReadFile(filepath.Join(base.BaseDir, "myapp", "version.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "2" {
		t.Errorf("version.txt = %q, want %q", string(data), "2")
	}
}
