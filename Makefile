.PHONY: test fmt fmt-check build build-release publish clean

FEATURES = scheduler,builtin-tools,reqwest-client,cache,cli

test:
	cargo test --features $(FEATURES)

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

build:
	cargo build --features cli

build-release:
	cargo build --features cli --release

# Publish to crates.io. Requires the user to be logged into the right
# Cloudflare account (wrangler OAuth) so the publish.sh helper can pull
# CRATES_IO_TOKEN from Secrets Store. See scripts/publish.sh for details.
publish:
	./scripts/publish.sh

clean:
	cargo clean
