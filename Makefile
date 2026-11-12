# Build, test, verify and run. Plain `make` runs the full verify: nothing
# should be considered shippable until it compiles, passes the tests, is
# formatted, and is clippy-clean.
#
# The whole verify uses one toolchain: the standalone Rust in $(RUST), which
# ships cargo, rustc, clippy-driver and rustfmt, all at the same version. The
# system rust is a newer cargo and rustc with no clippy-driver at all, so
# mixing them makes `cargo clippy` compile the dependencies with one version
# and lint the workspace with the other, and clippy then rejects every
# dependency with E0514. Putting $(RUST)/bin first in PATH keeps cargo, rustc,
# clippy-driver and rustfmt consistent. Any target can be run against a
# different toolchain: `make clippy RUST=/path/to/tonight`.

RUST ?= $(HOME)/.local/rust-1.99.0
CARGO ?= $(RUST)/bin/cargo
PATH := $(RUST)/bin:$(PATH)
PREFIX ?= $(HOME)/.local
# Needed for the toolchain's shared libraries (rustfmt and clippy-driver load
# librustc_driver). Harmless on a machine that does not need it.
LD_LIBRARY_PATH ?= $(RUST)/lib
BIN := wattdeck
RELEASE := target/release/$(BIN)
# Clippy writes to its own target dir, so a clippy run does not invalidate the
# build in ./target.
CLIPPY_TARGET ?= /tmp/opencode/clippy-target
# For the screenshots: an isolated HOME so the mock runs write nothing real, and
# a font for the renderer (empty means the renderer picks one it can find).
FAKEHOME ?= /tmp/wattdeck-screens
SCREENSHOTS_DIR := docs/screenshots
FONT ?=
PYTHON ?= python3

.DEFAULT_GOAL := all
.PHONY: all build release test test-install test-install-ps1 fmt fmt-check clippy run run-live run-debug \
        screenshots install uninstall clean help

all: build test fmt-check clippy ## full verify: build, test, fmt --check, clippy

build: ## cargo build (debug)
	$(CARGO) build

release: ## cargo build --release
	$(CARGO) build --release

test: ## the unit tests plus the CLI-surface test: no network, no device
	$(CARGO) test --all-targets

test-install: ## the install script's shell test (no network; needs make release)
	sh tests/install_sh.sh

test-install-ps1: ## install.ps1's test on PowerShell Core (no network; needs make release)
	sh tests/install_ps1.sh

fmt: ## rustfmt
	$(CARGO) fmt

fmt-check: ## rustfmt --check, fails on an unformatted tree
	LD_LIBRARY_PATH=$(LD_LIBRARY_PATH) $(CARGO) fmt --check

clippy: ## clippy --all-targets, in a separate target dir
	LD_LIBRARY_PATH=$(LD_LIBRARY_PATH) CARGO_TARGET_DIR=$(CLIPPY_TARGET) $(CARGO) clippy --all-targets

run: release ## run the release binary in mock mode, with no network
	./$(RELEASE) --mock

run-live: release ## run against the real devices; needs credentials.toml
	./$(RELEASE) --live

run-debug: release ## mock mode with the masked log file and verbose stderr
	./$(RELEASE) --mock --debug

screenshots: release ## render the README screenshots from the release binary
	@mkdir -p $(FAKEHOME) $(SCREENSHOTS_DIR) tests/fixtures
	$(PYTHON) tools/pty_probe.py 100 26 '' env HOME=$(FAKEHOME) ./$(RELEASE) --mock > tests/fixtures/frame_capture.txt
	@# `z` is unbound in the default key map, so each press drains 0.6 s without
	@# changing the screen: twelve presses let the mock sample, so the POWER
	@# HISTORY band has bars to chart. key 12 is that dashboard, key 13 the
	@# screen the 4/3 key switched to.
	@# The PNGs are not byte-reproducible across hours: the ALERTS lines carry
	@# the wall-clock time of each alert, and the hourly bucket label is the
	@# current hour. Regenerating on 2026-11-12 left dashboard.png pixel-identical
	@# and changed only the inside of those lines (difference bounding boxes
	@# alerts (29,77,87,127), analytics (449,97,457,107)), so the layout is the
	@# same and the committed images are what the target renders. Commit the
	@# output, not a claim.
	$(PYTHON) tools/pty_probe.py 100 26 'zzzzzzzzzzzz4q' env HOME=$(FAKEHOME) ./$(RELEASE) --mock > /tmp/wattdeck-alerts.txt
	$(PYTHON) tools/pty_probe.py 100 26 'zzzzzzzzzzzz3q' env HOME=$(FAKEHOME) ./$(RELEASE) --mock > /tmp/wattdeck-analytics.txt
	$(PYTHON) tools/render_frame.py /tmp/wattdeck-alerts.txt 'key 12' $(SCREENSHOTS_DIR)/dashboard.png $(if $(FONT),--font $(FONT))
	$(PYTHON) tools/render_frame.py /tmp/wattdeck-alerts.txt 'key 13' $(SCREENSHOTS_DIR)/alerts.png $(if $(FONT),--font $(FONT))
	$(PYTHON) tools/render_frame.py /tmp/wattdeck-analytics.txt 'key 13' $(SCREENSHOTS_DIR)/analytics.png $(if $(FONT),--font $(FONT))
	@echo "look at the three PNGs before committing them"

install: release ## install the release binary into $(PREFIX)/bin
	install -Dm755 $(RELEASE) $(PREFIX)/bin/$(BIN)
	@echo "installed $(PREFIX)/bin/$(BIN)"

uninstall: ## remove the installed binary
	rm -f $(PREFIX)/bin/$(BIN)

clean: ## cargo clean
	$(CARGO) clean

help: ## list the targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  %-12s %s\n", $$1, $$2}'
