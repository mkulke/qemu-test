GUEST_ASM = src/boot.asm
GUEST_BIN = payload/guest.bin
GUEST_PIO_STR_ASM = src/boot_pio_str.asm
GUEST_PIO_STR_BIN = payload/guest_pio_str.bin
VMLINUZ = payload/vmlinuz-virt
INITRD = payload/initrd.img
INIT_BIN = payload/init
INIT_SRC = src/lm_init.c
OS_IMAGE = payload/os-image.qcow2
OVMF_CODE = payload/OVMF_CODE.fd
ALPINE_URL = https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/x86_64/alpine-netboot-3.23.3-x86_64.tar.gz
UBUNTU_URL = https://cloud-images.ubuntu.com/minimal/releases/jammy/release/ubuntu-22.04-minimal-cloudimg-amd64.img
OVMF_DEB_URL = http://security.debian.org/debian-security/pool/updates/main/e/edk2/ovmf_2022.11-6+deb12u1_all.deb
QEMU_BIN ?= qemu-system-x86_64
REQUIRED_BUILD_TOOLS = cargo nasm wget gcc cpio gzip
REQUIRED_TOOLS = $(QEMU_BIN) ssh-keygen mkdosfs mcopy
BRIDGE_NAME = qemu-br0
BRIDGE_ADDR = 192.168.100.1/24
TAP_PREFIX = tap-qemu
NUM_TAPS ?= 2
RELEASE_BIN = target/release/qemu-test
RUST_SOURCES := $(shell find src -name "*.rs") build.rs Cargo.toml Cargo.lock
PAYLOADS = $(GUEST_BIN) \
		   $(GUEST_PIO_STR_BIN) \
		   $(VMLINUZ) \
		   $(INITRD) \
		   $(OS_IMAGE) \
		   $(OVMF_CODE)

.PHONY: build build-payloads build-release run run-release clean lint check-build-tools check-tools setup-bridge teardown-bridge

check-tools:
	@$(foreach tool,$(REQUIRED_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

check-build-tools:
	@$(foreach tool,$(REQUIRED_BUILD_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

build-payloads: check-build-tools $(PAYLOADS)

build: build-payloads
	cargo build
	cargo test

run: build check-tools
	cargo run

$(RELEASE_BIN): $(RUST_SOURCES) $(PAYLOADS)
	cargo build --release --locked

build-release: $(RELEASE_BIN)
	cargo test --release --locked

run-release: $(RELEASE_BIN) check-tools
	./$(RELEASE_BIN)

$(OVMF_CODE):
	cd payload && \
	wget -q $(OVMF_DEB_URL) -O ovmf.deb && \
	ar p ovmf.deb data.tar.xz | tar xJ --strip-components=4 ./usr/share/OVMF/OVMF_CODE.fd && \
	rm ovmf.deb

$(OS_IMAGE):
	wget -q $(UBUNTU_URL) -O $@

$(VMLINUZ):
	cd payload && \
	wget -q $(ALPINE_URL) -O - | tar xzf - boot/vmlinuz-virt --strip-components 1

$(GUEST_BIN): $(GUEST_ASM)
	nasm -f bin -o $@ $<

$(GUEST_PIO_STR_BIN): $(GUEST_PIO_STR_ASM)
	nasm -f bin -o $@ $<

$(INIT_BIN): $(INIT_SRC)
	gcc -static -o $@ $<

.DELETE_ON_ERROR:
$(INITRD): $(INIT_BIN)
	d=$$(mktemp -d) && \
	mkdir -p $$d/{dev,proc,sys} && \
	cp $< $$d/init && \
	(cd $$d && find . | cpio --quiet -o -H newc | gzip -9) > $@ && \
	rm -rf $$d

clean:
	rm -f $(PAYLOADS)
	cargo clean

lint:
	cargo fmt --check && \
	cargo clippy -- -D warnings

setup-bridge:
	ip link add $(BRIDGE_NAME) type bridge
	ip addr add $(BRIDGE_ADDR) dev $(BRIDGE_NAME)
	ip link set $(BRIDGE_NAME) up
	@echo "bridge $(BRIDGE_NAME) up with $(BRIDGE_ADDR)"
	@for i in $$(seq 0 $$(($(NUM_TAPS) - 1))); do \
		ip tuntap add dev $(TAP_PREFIX)-$$i mode tap user $$USER; \
		ip link set $(TAP_PREFIX)-$$i master $(BRIDGE_NAME); \
		ip link set $(TAP_PREFIX)-$$i up; \
		echo "tap $(TAP_PREFIX)-$$i up on $(BRIDGE_NAME)"; \
	done

teardown-bridge:
	@for tap in /sys/class/net/$(BRIDGE_NAME)/brif/*; do \
		name=$$(basename $$tap) && \
		ip link del $$name 2>/dev/null && \
		echo "tap $$name removed" || true; \
	done
	ip link del $(BRIDGE_NAME)
	@echo "bridge $(BRIDGE_NAME) removed"
