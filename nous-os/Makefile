# NOUS OS

CARGO ?= cargo
PREFIX ?= /usr
DESTDIR ?=

.PHONY: all build release test check clean install uninstall run iso fmt lint

all: build

build:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test

# What CI runs, and what you should run before pushing.
check: fmt lint test
	./target/debug/nousd --check

fmt:
	$(CARGO) fmt --check

lint:
	$(CARGO) clippy --all-targets -- -D warnings

# Start the daemon in the foreground with the graphical shell served on :7666.
run: release
	./target/release/nousd --http 7666

install: release
	install -Dm755 target/release/nousd   $(DESTDIR)$(PREFIX)/bin/nousd
	install -Dm755 target/release/nsh     $(DESTDIR)$(PREFIX)/bin/nsh
	install -Dm755 target/release/nousctl $(DESTDIR)$(PREFIX)/bin/nousctl
	install -Dm755 dist/overlay/usr/bin/nous-session $(DESTDIR)$(PREFIX)/bin/nous-session
	install -Dm755 dist/install.sh        $(DESTDIR)$(PREFIX)/bin/nous-install
	install -Dm644 dist/overlay/usr/lib/systemd/system/nousd.service \
		$(DESTDIR)$(PREFIX)/lib/systemd/system/nousd.service
	install -Dm644 dist/overlay/usr/lib/systemd/system/nous-shell.service \
		$(DESTDIR)$(PREFIX)/lib/systemd/system/nous-shell.service
	install -Dm644 dist/overlay/etc/nous/nous.conf $(DESTDIR)/etc/nous/nous.conf
	install -Dm644 dist/overlay/etc/nous/policy.d/10-desktop.conf \
		$(DESTDIR)/etc/nous/policy.d/10-desktop.conf
	@echo
	@echo "Installed. Start it with:  systemctl enable --now nousd"

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/{nousd,nsh,nousctl,nous-session,nous-install}
	rm -f $(DESTDIR)$(PREFIX)/lib/systemd/system/nous{d,-shell}.service
	@echo "Configuration in /etc/nous and state in /var/lib/nous were left alone."

iso: release
	sudo dist/build-iso.sh

clean:
	$(CARGO) clean
	rm -rf dist/out
