# cachy-auto-update: build and install
#
# Everything here is plain shell; "building" only means compiling the gettext
# catalogs and rendering the man page. Both are optional in the sense that the
# targets degrade to a no-op when msgfmt/scdoc are missing, so the tree stays
# usable for development without the build dependencies installed.

# Overridable so a packager can pass the version it is actually building
# (`make VERSION=$pkgver`). The literal below is the fallback for builds
# straight from a checkout, and is what a release tag has to carry.
VERSION      ?= 1.3.0

PREFIX       ?= /usr
DESTDIR      ?=
BINDIR       ?= $(PREFIX)/bin
LIBEXECDIR   ?= $(PREFIX)/lib/cachy-auto-update
DATADIR      ?= $(PREFIX)/share
LIBDIR       ?= $(DATADIR)/cachy-auto-update/lib
LOCALEDIR    ?= $(DATADIR)/locale
MANDIR       ?= $(DATADIR)/man
SYSCONFDIR   ?= /etc
SYSTEMDDIR   ?= $(PREFIX)/lib/systemd/system
SYSUSERSDIR  ?= $(PREFIX)/lib/sysusers.d
TMPFILESDIR  ?= $(PREFIX)/lib/tmpfiles.d
XDGAUTOSTART ?= $(SYSCONFDIR)/xdg/autostart

LINGUAS      := de
MOFILES      := $(patsubst %,po/%.mo,$(LINGUAS))
MANPAGE      := doc/cachy-auto-update.1

LIBS         := $(wildcard src/lib/*.sh)

MSGFMT       := $(shell command -v msgfmt 2>/dev/null)
SCDOC        := $(shell command -v scdoc 2>/dev/null)

.PHONY: all build install uninstall check clean

all: build

build: $(MOFILES) $(MANPAGE)

po/%.mo: po/%.po
ifdef MSGFMT
	$(MSGFMT) --check --output-file=$@ $<
else
	@echo "msgfmt not found, skipping $@"
endif

$(MANPAGE): doc/cachy-auto-update.1.scd
ifdef SCDOC
	$(SCDOC) < $< > $@
else
	@echo "scdoc not found, skipping $@"
endif

# Syntax-check every shell file, and run shellcheck when it is available.
check:
	@set -e; for f in src/cachy-auto-update src/cachy-auto-update-run $(LIBS); do \
		bash -n "$$f" && echo "ok  $$f"; \
	done
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck -x -e SC1090,SC1091 src/cachy-auto-update src/cachy-auto-update-run $(LIBS); \
		echo "ok  shellcheck"; \
	else \
		echo "shellcheck not found, skipped"; \
	fi
	@if command -v python3 >/dev/null 2>&1; then \
		python3 -m py_compile src/cachy-auto-update-progress \
			&& echo "ok  src/cachy-auto-update-progress"; \
		rm -rf src/__pycache__; \
	else \
		echo "python3 not found, skipped"; \
	fi
	@if command -v visudo >/dev/null 2>&1; then \
		visudo -cf res/sudoers/cachy-auto-update >/dev/null && echo "ok  sudoers"; \
	fi

install: build
	# executables
	install -Dm755 src/cachy-auto-update     "$(DESTDIR)$(BINDIR)/cachy-auto-update"
	install -Dm755 src/cachy-auto-update-run "$(DESTDIR)$(LIBEXECDIR)/cachy-auto-update-run"
	# holds a session bus connection open so the update can drive a progress bar
	install -Dm755 src/cachy-auto-update-progress \
		"$(DESTDIR)$(LIBEXECDIR)/cachy-auto-update-progress"
	# the version and the resolved lib path are baked in at install time
	sed -i -e 's|@VERSION@|$(VERSION)|g' \
	       -e 's|@LIBDIR@|$(LIBDIR)|g' \
	       -e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' \
	       -e 's|@LOCALEDIR@|$(LOCALEDIR)|g' \
	       "$(DESTDIR)$(BINDIR)/cachy-auto-update" \
	       "$(DESTDIR)$(LIBEXECDIR)/cachy-auto-update-run"

	# shell libraries
	install -d "$(DESTDIR)$(LIBDIR)"
	install -Dm644 -t "$(DESTDIR)$(LIBDIR)" $(LIBS)
	sed -i -e 's|@VERSION@|$(VERSION)|g' \
	       -e 's|@LIBDIR@|$(LIBDIR)|g' \
	       -e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' \
	       -e 's|@LOCALEDIR@|$(LOCALEDIR)|g' \
	       "$(DESTDIR)$(LIBDIR)"/*.sh

	# configuration (marked as backup= in the PKGBUILD)
	install -Dm644 res/config/cachy-auto-update.conf \
		"$(DESTDIR)$(SYSCONFDIR)/cachy-auto-update/cachy-auto-update.conf"
	install -Dm644 res/logrotate/cachy-auto-update \
		"$(DESTDIR)$(SYSCONFDIR)/logrotate.d/cachy-auto-update"

	# privilege bridge for the AUR helper: a locked system account plus a
	# NOPASSWD rule scoped to pacman only
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

	# delivers notifications that were queued while nobody was logged in
	install -Dm644 res/autostart/cachy-auto-update-notify.desktop \
		"$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-notify.desktop"

	# names and illustrates the progress bar; hidden from the application menu
	install -Dm644 res/applications/cachy-auto-update.desktop \
		"$(DESTDIR)$(DATADIR)/applications/cachy-auto-update.desktop"

	# translations
	@for l in $(LINGUAS); do \
		if [ -f "po/$$l.mo" ]; then \
			install -Dm644 "po/$$l.mo" \
				"$(DESTDIR)$(LOCALEDIR)/$$l/LC_MESSAGES/cachy-auto-update.mo"; \
		fi; \
	done

	# documentation
	@if [ -f $(MANPAGE) ]; then \
		install -Dm644 $(MANPAGE) "$(DESTDIR)$(MANDIR)/man1/cachy-auto-update.1"; \
	fi
	install -Dm644 README.md "$(DESTDIR)$(DATADIR)/doc/cachy-auto-update/README.md"

uninstall:
	rm -f  "$(DESTDIR)$(BINDIR)/cachy-auto-update"
	rm -rf "$(DESTDIR)$(LIBEXECDIR)"
	rm -rf "$(DESTDIR)$(DATADIR)/cachy-auto-update"
	rm -f  "$(DESTDIR)$(SYSCONFDIR)/sudoers.d/cachy-auto-update"
	rm -f  "$(DESTDIR)$(SYSUSERSDIR)/cachy-auto-update.conf"
	rm -f  "$(DESTDIR)$(TMPFILESDIR)/cachy-auto-update.conf"
	rm -f  "$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.service"
	rm -f  "$(DESTDIR)$(SYSTEMDDIR)/cachy-auto-update.timer"
	rm -f  "$(DESTDIR)$(XDGAUTOSTART)/cachy-auto-update-notify.desktop"
	rm -f  "$(DESTDIR)$(DATADIR)/applications/cachy-auto-update.desktop"
	rm -f  "$(DESTDIR)$(MANDIR)/man1/cachy-auto-update.1"

clean:
	rm -f po/*.mo $(MANPAGE)
