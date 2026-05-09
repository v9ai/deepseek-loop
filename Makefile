.PHONY: test fmt fmt-check build build-release publish repo-sync clean

FEATURES = scheduler,builtin-tools,reqwest-client,cache,cli
GITHUB_REPO = v9ai/deepseek-loop
GITHUB_TOPICS = rust deepseek agent llm claude-code cron-scheduler tool-use cli

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

# Sync GitHub repo metadata (description, homepage, topics) from Cargo.toml.
# Runs automatically at the tail of `make publish`; this target lets you sync
# without re-publishing.
repo-sync:
	@gh repo edit $(GITHUB_REPO) \
		--description "$$(awk -F'"' '/^description =/ {print $$2; exit}' Cargo.toml)" \
		--homepage "https://crates.io/crates/deepseek-loop" \
		$(foreach t,$(GITHUB_TOPICS),--add-topic $(t))

clean:
	cargo clean
