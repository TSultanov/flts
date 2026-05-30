// Package main is the FLTS Syncthing engine wrapper. It is compiled with
// `go build -buildmode=c-archive` into a static library (.a + .h) that the
// `syncthing-sys` Rust crate links and calls over a tiny C ABI.
//
// The surface is deliberately minimal — start/stop/ping only. All real control
// (devices, folders, status) happens from Rust over the engine's localhost REST
// API, keeping this fragile Go↔native bridge as small as possible.
//
// API note: this targets Syncthing v1.30.0. The `lib/syncthing` startup API is
// version-sensitive; the sequence below mirrors upstream `cmd/syncthing`
// (earlyService serving evLogger + cfg wrapper, Modify the GUI binding, then
// New + Start).
package main

// NOTE: a comment placed immediately above `import "C"` (no blank line) is
// treated by cgo as the C preamble, so this note is intentionally separated
// from the import by a blank line.

import "C"

import (
	"context"
	"path/filepath"
	"sync"

	"github.com/syncthing/syncthing/lib/config"
	"github.com/syncthing/syncthing/lib/events"
	"github.com/syncthing/syncthing/lib/locations"
	"github.com/syncthing/syncthing/lib/svcutil"
	"github.com/syncthing/syncthing/lib/syncthing"
	"github.com/thejerf/suture/v4"
)

// engine holds the live process state. Guarded by mu; nil when stopped.
type engine struct {
	app          *syncthing.App
	earlyCancel  context.CancelFunc
}

var (
	mu      sync.Mutex
	running *engine
)

//export flts_st_ping
//
// flts_st_ping returns a fixed sentinel so the Rust side can assert the FFI
// chain is live without standing up the full engine.
func flts_st_ping() C.int {
	return 4711
}

//export flts_st_start
//
// flts_st_start brings up the embedded Syncthing engine with its home (certs,
// config.xml, index DB) under `home`, the REST/GUI bound to `guiAddr`
// (e.g. "127.0.0.1:0" or a fixed port) and authenticated by `apiKey`.
//
// When `hermetic` is non-zero the engine stays fully local: no public/LAN
// discovery, no relays, no NAT, and a random loopback BEP port. Used by tests
// and the Docker harness so they never announce throwaway devices to the public
// network or collide on the default port 22000. (It is passed as a parameter
// rather than read from an env var because the Go runtime snapshots the
// environment at c-archive init, before the Rust caller could set it.)
//
// Returns 0 on success (engine started, REST listening) or a small non-zero
// code identifying the failing step. Idempotent: a second call while running
// is a no-op success.
func flts_st_start(home, guiAddr, apiKey *C.char, hermetic C.int) C.int {
	mu.Lock()
	defer mu.Unlock()
	if running != nil {
		return 0
	}

	homeDir := C.GoString(home)
	addr := C.GoString(guiAddr)
	key := C.GoString(apiKey)

	certFile := filepath.Join(homeDir, "cert.pem")
	keyFile := filepath.Join(homeDir, "key.pem")
	configPath := filepath.Join(homeDir, "config.xml")
	dbPath := filepath.Join(homeDir, locations.LevelDBDir)

	// earlyService runs the services that must be live before/around app
	// startup: the event logger and the config wrapper (whose Serve loop is
	// what makes Modify below actually apply). Mirrors upstream cmd/syncthing.
	earlyCtx, earlyCancel := context.WithCancel(context.Background())
	earlyService := suture.New("flts-early", suture.Spec{})
	earlyService.ServeBackground(earlyCtx)

	evLogger := events.NewLogger()
	earlyService.Add(evLogger)

	cert, err := syncthing.LoadOrGenerateCertificate(certFile, keyFile)
	if err != nil {
		earlyCancel()
		return 2
	}

	// allowNewerConfig=true (don't refuse a config from a newer build),
	// noDefaultFolder=true (FLTS manages its own folder), skipPortProbing=true
	// (we set the GUI address explicitly; no need to probe).
	cfg, err := syncthing.LoadConfigAtStartup(configPath, cert, evLogger, true, true, true)
	if err != nil {
		earlyCancel()
		return 3
	}
	earlyService.Add(cfg)

	isHermetic := hermetic != 0

	// Bind the REST/GUI to the requested localhost address + API key. This must
	// happen before app.Start(), which stands up the GUI during startup.
	waiter, err := cfg.Modify(func(c *config.Configuration) {
		c.GUI.Enabled = true
		c.GUI.RawAddress = addr
		c.GUI.APIKey = key
		c.GUI.RawUseTLS = false
		if isHermetic {
			c.Options.GlobalAnnEnabled = false
			c.Options.LocalAnnEnabled = false
			c.Options.RelaysEnabled = false
			c.Options.NATEnabled = false
			c.Options.RawListenAddresses = []string{"tcp://127.0.0.1:0"}
		}
	})
	if err != nil {
		earlyCancel()
		return 4
	}
	waiter.Wait()

	dbBackend, err := syncthing.OpenDBBackend(dbPath, cfg.Options().DatabaseTuning)
	if err != nil {
		earlyCancel()
		return 5
	}

	app, err := syncthing.New(cfg, dbBackend, evLogger, cert, syncthing.Options{NoUpgrade: true})
	if err != nil {
		earlyCancel()
		return 6
	}
	if err := app.Start(); err != nil {
		earlyCancel()
		return 7
	}

	running = &engine{app: app, earlyCancel: earlyCancel}
	return 0
}

//export flts_st_stop
//
// flts_st_stop stops the engine cleanly and tears down the early services.
// Idempotent: a no-op success when nothing is running.
func flts_st_stop() C.int {
	mu.Lock()
	defer mu.Unlock()
	if running == nil {
		return 0
	}
	running.app.Stop(svcutil.ExitSuccess)
	running.earlyCancel()
	running = nil
	return 0
}

// main is required by `package main` but is never invoked in c-archive mode.
func main() {}
