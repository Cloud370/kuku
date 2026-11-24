.PHONY: build-web build build-all release-linux clean

build-web:
	cd apps/web && npm ci && npm run build

build:
	cargo build --locked --release

build-all:
	cargo build --locked --release --target x86_64-unknown-linux-gnu
	cargo build --locked --release --target aarch64-apple-darwin
	cargo build --locked --release --target x86_64-pc-windows-msvc

release-linux:
	cross build --locked --release --target x86_64-unknown-linux-musl

clean:
	cargo clean
	rm -rf apps/web/dist
