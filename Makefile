.PHONY: build-web build build-all release-linux clean

build-web:
	cd apps/web && npm install && npm run build

build: build-web
	cargo build --release

build-all: build-web
	cargo build --release --target x86_64-unknown-linux-gnu
	cargo build --release --target aarch64-apple-darwin
	cargo build --release --target x86_64-pc-windows-msvc

release-linux: build-web
	cross build --release --target x86_64-unknown-linux-musl

clean:
	cargo clean
	rm -rf apps/web/dist
