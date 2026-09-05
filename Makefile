BINARY     := $(shell . ./script/lib && echo $$BINARY)
VERSION    := $(shell . ./script/lib && echo $$VERSION)
ICON_SIZES := $(shell . ./script/lib && echo $$ICON_SIZES)

ifeq ($(DESTDIR),)
SUDO := $(shell [ -w /usr/bin ] || echo sudo)
endif

.PHONY: build install icons dist release release-github release-aur clean

build:
	cargo fetch --locked
	cargo build --release --package $(BINARY)

# Regenerates assets/icons from assets/icon.png. Its output is committed, so this is only run
# when the icon changes — nothing in a build or a package depends on it.
icons:
	./script/icons

install: build
	$(SUDO) install -Dm755 target/release/$(BINARY) $(DESTDIR)/usr/bin/$(BINARY)
	$(SUDO) install -Dm644 packaging/$(BINARY).desktop $(DESTDIR)/usr/share/applications/$(BINARY).desktop
	@set -e; for size in $(ICON_SIZES); do \
		$(SUDO) install -Dm644 assets/icons/$(BINARY)-$$size.png \
			$(DESTDIR)/usr/share/icons/hicolor/$${size}x$${size}/apps/$(BINARY).png; \
	done

# Only the host architecture, and deliberately no cross-compilation: GTK and libadwaita are
# linked dynamically, and cross-building would mean a cross copy of both and everything under
# them, for the one architecture anybody has asked for so far.
dist: build
	mkdir -p dist
	cp target/release/$(BINARY) dist/$(BINARY)-linux-amd64

release: dist release-github release-aur
	@echo
	@echo "Released v$(VERSION)"
	@echo

release-github:
	./script/release-github

release-aur:
	./script/release-aur

clean:
	cargo clean
	rm -rf dist
	rm -rf target/aur
