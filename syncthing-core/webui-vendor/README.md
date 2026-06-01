# Syncthing Web-UI vendor assets

Third-party frontend libraries (Angular, Bootstrap, jQuery, moment, fancytree,
fork-awesome, daterangepicker, HumanizeDuration) for Syncthing's built-in web
dashboard, taken verbatim from `gui/default/vendor/` of the pinned Syncthing
release (`v1.30.0`, matching `../go.mod`).

## Why this is committed

The dashboard is embedded **in debug builds only** (see `../../syncthing-sys/build.rs`),
so developers can open it from the sync settings panel. Syncthing's Go module
ships the GUI source but *not* these `vendor/` libs: Go's module-zip packaging
strips nested directories named `vendor/`, so they never reach the module cache.
We therefore vendor them here and `build.rs` copies this tree into the generated
`gui/default/vendor/` before running Syncthing's `genassets.go`.

## Updating (when bumping the Syncthing version in go.mod)

```sh
curl -sL -o /tmp/st.tgz \
  "https://codeload.github.com/syncthing/syncthing/tar.gz/refs/tags/v<VERSION>"
rm -rf ./* && \
  tar -xzf /tmp/st.tgz --strip-components=4 'syncthing-*/gui/default/vendor'
```

Each library keeps its upstream LICENSE file; all are MPL/MIT/Apache-style and
redistributable.
