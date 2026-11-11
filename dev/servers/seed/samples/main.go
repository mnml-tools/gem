// Command api serves the synthetic demo API.
package main

import (
	"context"
	"log"
	"net/http"
	"os"
	"os/signal"
	"time"
)

type server struct {
	addr    string
	timeout time.Duration
}

func (s *server) health(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write([]byte(`{"status":"ok"}`))
}

func main() {
	s := &server{addr: ":8080", timeout: 5 * time.Second}
	mux := http.NewServeMux()
	mux.HandleFunc("/health", s.health)

	srv := &http.Server{Addr: s.addr, Handler: mux, ReadHeaderTimeout: s.timeout}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	go func() {
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("listen: %v", err)
		}
	}()
	<-ctx.Done()
	_ = srv.Shutdown(context.Background())
}
