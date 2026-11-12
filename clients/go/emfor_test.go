package emfor

import (
	"errors"
	"os"
	"sync"
	"testing"
)

// The Makefile writes clients/c/tests/fixture.json to EMFOR_STORE: service
// api.svc.emfor, round-robin over these two backends.
var backends = map[string]bool{"10.0.0.11:8080": true, "10.0.0.12:8080": true}

func needStore(t *testing.T) {
	if os.Getenv("EMFOR_STORE") == "" {
		t.Skip("EMFOR_STORE not set; run make test")
	}
}

func TestLookupReturnsABackend(t *testing.T) {
	needStore(t)
	a, err := Lookup("api.svc.emfor", "")
	if err != nil || !backends[a] {
		t.Fatalf("Lookup = %q, %v", a, err)
	}
	b, err := Lookup("API.svc.emfor.", "user-42")
	if err != nil || !backends[b] || a == b {
		t.Fatalf("second Lookup = %q, %v; first %q (want the other backend)", b, err, a)
	}
}

func TestNotFoundIsAnError(t *testing.T) {
	needStore(t)
	_, err := Lookup("nope.svc.emfor", "")
	var e *Error
	if !errors.As(err, &e) || e.Code != NotFound || e.Code.String() != "NOT_FOUND" || e.Message == "" {
		t.Fatalf("err = %#v", err)
	}
	if err.Error() != "NOT_FOUND: "+e.Message {
		t.Fatalf("Error() = %q", err.Error())
	}
}

func TestInvalidArgument(t *testing.T) {
	for _, args := range [][2]string{{"api\x00svc", ""}, {"api.svc.emfor", "a\x00b"}, {"\xff", ""}} {
		_, err := Lookup(args[0], args[1])
		var e *Error
		if err == nil || errors.As(err, &e) {
			t.Fatalf("Lookup(%q, %q) err = %#v, want a plain error", args[0], args[1], err)
		}
	}
}

func TestCodeNames(t *testing.T) {
	want := map[Code]string{Unavailable: "UNAVAILABLE", Incompatible: "INCOMPATIBLE", NotFound: "NOT_FOUND", AllDown: "ALL_DOWN", 9: "UNKNOWN"}
	for c, s := range want {
		if c.String() != s {
			t.Errorf("Code(%d).String() = %q, want %q", int(c), c.String(), s)
		}
	}
}

func TestConcurrentLookups(t *testing.T) {
	needStore(t)
	const workers, each = 8, 1000
	var mu sync.Mutex
	counts := map[string]int{}
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			local := map[string]int{}
			for j := 0; j < each; j++ {
				a, err := Lookup("api.svc.emfor", "")
				if err != nil {
					t.Error(err)
					return
				}
				local[a]++
			}
			mu.Lock()
			for k, v := range local {
				counts[k] += v
			}
			mu.Unlock()
		}()
	}
	wg.Wait()
	if counts["10.0.0.11:8080"] != workers*each/2 || counts["10.0.0.12:8080"] != workers*each/2 {
		t.Fatalf("counts = %v, want %d each", counts, workers*each/2)
	}
}
