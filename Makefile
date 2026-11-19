# cachy-auto-update: build and install
#
# Two programs, built with cargo: cachy-auto-update (the command line and the
# update run) and cachy-auto-update-gui (the window and the tray icon). The
# translations in po/ are compiled into both. The man page needs scdoc and is
# skipped without it.

# The version comes from Cargo.toml. VERSION is still taken, so an older
# PKGBUILD that passes it keeps working.
VERSION      ?=

PREFIX       ?= /usr
DESTDIR      ?=
BINDIR       ?= $(PREFIX)/bin
DATADIR      ?= $(PREFIX)/share
MANDIR       ?= $(DATADIR)/man
SYSCONFDIR   ?= /etc
SYSTEMDDIR   ?= $(PREFIX)/lib/systemd/system
SYSUSERSDIR  ?= $(PREFIX)/lib/sysusers.d
TMPFILESDIR  ?= $(PREFIX)/lib/tmpfiles.d
XDGAUTOSTART ?= $(SYSCONFDIR)/xdg/autostart
POLKITRULES  ?= $(DATADIR)/polkit-1/rules.d
POLKITACTIONS ?= $(DATADIR)/polkit-1/actions

CARGO        ?= cargo
CARGOFLAGS   ?= --release --locked
TARGET       := target/release

MANPAGE      := doc/cachy-auto-update.1
SCDOC        := $(shell command -v scdoc 2>/dev/null)

.PHONY: all build install uninstall check clean

all: build

build: $(MANPAGE)
	$(CARGO) build $(CARGOFLAGS) --workspace

$(MANPAGE): doc/cachy-auto-update.1.scd
ifdef SCDOC
	$(SCDOC) < $< > $@
else
	@echo "scdoc not found, skipping $@"
endif

check:
	$(CARGO) test --workspace
	$(CARGO) clippy --workspace -- -D warnings
	@if command -v visudo >/dev/null 2>&1; then \
		visudo -cf res/sudoers/cachy-auto-update >/dev/null && echo "ok  sudoers"; \
	fi

install:
	install -Dm755 $(TARGET)/cachy-auto-update     "$(DESTDIR)$(BINDIR)/cachy-auto-update"
	install -Dm755 $(TARGET)/cachy-auto-update-gui "$(DESTDIR)$(BINDIR)/cachy-auto-update-gui"

	# configuration (marked as backup= in the PKGBUILD). An existing one is
	# kept: the new defaults go next to it as .new.
	@conf="$(DESTDIR)$(SYSCONFDIR)/cachy-auto-update/cachy-auto-update.conf"; \
	if [ -e "$$conf" ] && ! cmp -s res/config/cachy-auto-update.conf "$$conf"; then \
		install -Dm644 res/config/cachy-auto-update.conf "$$conf.new"; \
		echo "kept $$conf, the new defaults are in $$conf.new"; \
	else \
		install -Dm644 res/config/cachy-auto-update.conf "$$conf"; \
	fi
	install -Dm644 res/logrotate/cachy-auto-update \
		"$(DESTDIR)$(SYSCONFDIR)/logrotate.d/cachy-auto-update"

	# privilege bridge for the AUR helper: a locked system account plus a
	# NOPASSWD rule scoped to pacman only. The folder keeps sudo's own 750,
	# else pacman warns that it differs from the one on disk.
	install -dm750 "$(DESTDIR)$(SYSCONFDIR)/sudoers.d"
	install -Dm440 res/sudoers/cachy-auto-update \
		"$(DESTDIR)$(SYSCONFDIR)/sudoers.d/cachy-auto-update"
	install -Dm644 res/sysusers/cachy-auto-update.conf \
		"$(DESTDIR)$(SYSUSERSDIR)/cachy-auto-update.conf"
	install -Dm644 res/tmpfiles/cachy-auto-update.conf \
		"$(DESTDIR)$(TMPFILESDIR)/cachy-auto-update.conf"

	# system units
	install -Dm644 res/systemd/cachy-auto-update.service \
		"$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.service"
	install -Dm644 res/systemd/cachy-auto-update.timer \
		"$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.timer"
	# the update Update now starts, and the rule that lets it without a
	# password
	install -Dm644 res/systemd/cachy-auto-update-now.service \
		"$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update-now.service"
	install -Dm644 res/polkit/cachy-auto-update.rules \
		"$(DESTDIR)$(POLKITRULES)/cachy-auto-update.rules"
	# lets the window change settings through pkexec
	install -Dm644 res/polkit/io.github.loonixtools.cachy-auto-update.policy \
		"$(DESTDIR)$(POLKITACTIONS)/io.github.loonixtools.cachy-auto-update.policy"

	# delivers notifications that were queued while nobody was logged in,
	# and starts the tray icon
	install -Dm644 res/autostart/cachy-auto-update-notify.desktop \
		"$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-notify.desktop"
	install -Dm644 res/autostart/cachy-auto-update-tray.desktop \
		"$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-tray.desktop"

	# the window in the application menu; it also names the progress bar
	install -Dm644 res/applications/cachy-auto-update.desktop \
		"$(DESTDIR)$(DATADIR)/applications/cachy-auto-update.desktop"
	# the app's icon, for the menu, the window, notifications and polkit
	install -Dm644 res/cachy-auto-update.svg \
		"$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/cachy-auto-update.svg"

	# documentation
	@if [ -f $(MANPAGE) ]; then \
		install -Dm644 $(MANPAGE) "$(DESTDIR)$(MANDIR)/man1/cachy-auto-update.1"; \
	fi
	install -Dm644 README.md "$(DESTDIR)$(DATADIR)/doc/cachy-auto-update/README.md"

uninstall:
	rm -f  "$(DESTDIR)$(BINDIR)/cachy-auto-update"
	rm -f  "$(DESTDIR)$(BINDIR)/cachy-auto-update-gui"
	rm -f  "$(DESTDIR)$(SYSCONFDIR)/sudoers.d/cachy-auto-update"
	rm -f  "$(DESTDIR)$(SYSUSERSDIR)/cachy-auto-update.conf"
	rm -f  "$(DESTDIR)$(TMPFILESDIR)/cachy-auto-update.conf"
	rm -f  "$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.service"
	rm -f  "$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.timer"
	rm -f  "$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update-now.service"
	rm -f  "$(DESTDIR)$(POLKITRULES)/cachy-auto-update.rules"
	rm -f  "$(DESTDIR)$(POLKITACTIONS)/io.github.loonixtools.cachy-auto-update.policy"
	rm -f  "$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-notify.desktop"
	rm -f  "$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-tray.desktop"
	rm -f  "$(DESTDIR)$(DATADIR)/applications/cachy-auto-update.desktop"
	rm -f  "$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/cachy-auto-update.svg"
	rm -f  "$(DESTDIR)$(MANDIR)/man1/cachy-auto-update.1"
	# left by versions before 2.0
	rm -rf "$(DESTDIR)$(PREFIX)/lib/cachy-auto-update"
	rm -rf "$(DESTDIR)$(DATADIR)/cachy-auto-update"

clean:
	$(CARGO) clean
	rm -f $(MANPAGE)
