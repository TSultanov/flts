// Package main is the FLTS Syncthing engine wrapper, built with
// `go build -buildmode=c-archive` and linked by the `syncthing-sys` Rust crate.
//
// Keep the C ABI to start/stop/ping; all other control goes from Rust over the
// engine's localhost REST API. Targets Syncthing v1.30.0, whose startup API is
// version-sensitive: the sequence below mirrors upstream `cmd/syncthing`.
package main

// Never let a comment touch `import "C"` — cgo would read it as the C preamble.

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
// Returns a fixed sentinel so Rust can assert the FFI chain is live.
func flts_st_ping() C.int {
	return 4711
}

//export flts_st_start
//
// Starts the engine: state under `home`, REST/GUI on `guiAddr` keyed by
// `apiKey`. Non-zero `hermetic` disables discovery/relays/NAT and uses a random
// loopback BEP port; it must be a parameter, not an env var, because the Go
// runtime snapshots the environment at c-archive init.
//
// Returns 0, or a non-zero code identifying the failing step. Idempotent.
func flts_st_start(home, guiAddr, apiKey *C.char, hermetic C.int) C.int {
	mu.Lock()
	defer mu.Unlock()
	if running != nil {
		return 0
	}

	homeDir := C.GoString(home)
	addr := C.GoString(guiAddr)
	key := C.GoString(apiKey)

	// Some paths (e.g. the GUI cert) resolve through these global base dirs, not
	// the explicit ones below; without this they leak to a possibly-absent XDG dir.
	_ = locations.SetBaseDir(locations.ConfigBaseDir, homeDir)
	_ = locations.SetBaseDir(locations.DataBaseDir, homeDir)

	certFile := filepath.Join(homeDir, "cert.pem")
	keyFile := filepath.Join(homeDir, "key.pem")
	configPath := filepath.Join(homeDir, "config.xml")
	dbPath := filepath.Join(homeDir, locations.LevelDBDir)

	// The config wrapper's Serve loop is what makes Modify below apply, so it and
	// the event logger must run before app startup.
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

	// allowNewerConfig, noDefaultFolder (FLTS owns its folder), skipPortProbing.
	cfg, err := syncthing.LoadConfigAtStartup(configPath, cert, evLogger, true, true, true)
	if err != nil {
		earlyCancel()
		return 3
	}
	earlyService.Add(cfg)

	isHermetic := hermetic != 0

	// Must precede app.Start(), which stands up the GUI.
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
// Stops the engine and the early services. Idempotent.
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

// Required by `package main`; never invoked in c-archive mode.
func main() {}
