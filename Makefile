.PHONY: build release clean fmt fmt-check clippy doc

UNAME_S := $(shell uname -s)
FEATURES ?=

ifeq ($(UNAME_S),Darwin)
    build:
		cargo build --target aarch64-unknown-linux-gnu $(if $(FEATURES),--features $(FEATURES))
    release:
		cargo build --release --target aarch64-unknown-linux-gnu $(if $(FEATURES),--features $(FEATURES))
    clippy:
		cargo clippy --target aarch64-unknown-linux-gnu $(if $(FEATURES),--features $(FEATURES))
    doc:
		cargo doc --target aarch64-unknown-linux-gnu --no-deps
else
    build:
		cargo build $(if $(FEATURES),--features $(FEATURES))
    release:
		cargo build --release $(if $(FEATURES),--features $(FEATURES))
    clippy:
		cargo clippy $(if $(FEATURES),--features $(FEATURES))
    doc:
		cargo doc --no-deps --open
endif

fmt:
	cargo fmt

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean
