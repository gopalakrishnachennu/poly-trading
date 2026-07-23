# Local/CI parity. `make verify` runs the same gates as .github/workflows/verify.yml.
# Research helpers (capture/analyze/backtest) wrap scripts/ tools.

.DEFAULT_GOAL := help
SHELL := /bin/sh

TLA_JAR     := formal/.tla2tools.jar
TLA_URL     := https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
TLA_SHA     := cc4803dce2a8ffaf0f5920a9dc39df4b5ee34ab4cb53fb58ac557277a7e516b3

.PHONY: help verify ci fmt fmt-fix clippy test deny terminal terminal-install \
        tla capture progress backtest analyze clean

help: ## Show this help
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

verify: fmt clippy test deny terminal ## Run all fast gates (mirrors CI, minus TLA)

ci: verify tla ## Run every gate including all TLA+ models

fmt: ## Check Rust formatting
	cargo fmt --check

fmt-fix: ## Apply Rust formatting
	cargo fmt

clippy: ## Lint Rust with warnings denied
	cargo clippy --workspace --all-targets -- -D warnings

test: ## Run the Rust workspace tests
	cargo test --workspace --locked

deny: ## Audit dependencies (licenses, advisories, sources)
	@command -v cargo-deny >/dev/null 2>&1 || { \
		echo "cargo-deny not installed: cargo install cargo-deny --locked"; exit 1; }
	cargo deny --workspace check

terminal-install: ## Install terminal dependencies
	cd terminal && npm ci

terminal: ## Lint and render-test the operator terminal
	cd terminal && npm run lint && npm test

$(TLA_JAR): ## Download and verify the pinned TLC jar
	@echo "downloading pinned TLC jar..."
	@curl --fail --location --silent --show-error --output $(TLA_JAR) $(TLA_URL)
	@got=`(sha256sum $(TLA_JAR) 2>/dev/null || shasum -a 256 $(TLA_JAR)) | awk '{print $$1}'`; \
	if [ "$$got" != "$(TLA_SHA)" ]; then \
		echo "TLA jar checksum mismatch: got $$got"; rm -f $(TLA_JAR); exit 1; fi; \
	echo "TLC jar verified."

tla: $(TLA_JAR) ## Check every bounded TLA+ model
	@cd formal && for cfg in *.cfg; do \
		case "$$cfg" in *TTrace*) continue;; esac; \
		model="$${cfg%.cfg}"; \
		[ -f "$$model.tla" ] || continue; \
		echo "== $$model =="; \
		java -cp ../$(TLA_JAR) tlc2.TLC -config "$$cfg" "$$model.tla" || exit 1; \
	done

capture: ## Start unattended supervised research capture (gateway + recorder)
	scripts/run-continuous-capture.sh

progress: ## Report accumulated resolved markets for backtesting
	python3 scripts/capture_progress.py

analyze: ## Complete-set arbitrage opportunity scan over captured data
	python3 scripts/analyze_paper_edge.py

backtest: ## Directional fair-value walk-forward backtest with sensitivity sweep
	python3 scripts/backtest_fair_value.py --walk-forward --sweep

clean: ## Remove Rust build artifacts and the downloaded TLC jar
	cargo clean
	rm -f $(TLA_JAR)
