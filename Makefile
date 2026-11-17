.PHONY: build-web build build-all release-linux clean

build-web:
	cd apps/web && npm ci && npm run build

build: build-web
	cargo build --locked --release --features embedded-web-assets

build-all: build-web
	cargo build --locked --release --features embedded-web-assets --target x86_64-unknown-linux-gnu
	cargo build --locked --release --features embedded-web-assets --target aarch64-apple-darwin
	cargo build --locked --release --features embedded-web-assets --target x86_64-pc-windows-msvc

release-linux: build-web
	cross build --locked --release --features embedded-web-assets --target x86_64-unknown-linux-musl

clean:
	cargo clean
	rm -rf apps/web/dist
