GUEST_ASM = src/boot.asm
GUEST_BIN = payload/guest.bin
VMLINUZ = payload/vmlinuz-virt
OS_IMAGE = payload/os-image.qcow2
ALPINE_URL = https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/x86_64/alpine-netboot-3.23.3-x86_64.tar.gz
UBUNTU_URL = https://cloud-images.ubuntu.com/minimal/releases/jammy/release/ubuntu-22.04-minimal-cloudimg-amd64.img
QEMU_BIN ?= qemu-system-x86_64
REQUIRED_TOOLS = cargo nasm wget $(QEMU_BIN) ssh-keygen mkdosfs mcopy

.PHONY: build run clean lint check-tools

check-tools:
	@$(foreach tool,$(REQUIRED_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

build: check-tools $(GUEST_BIN) $(VMLINUZ) $(OS_IMAGE)
	cargo build

run: build
	cargo run

$(OS_IMAGE):
	wget -q $(UBUNTU_URL) -O $@

$(VMLINUZ):
	cd payload && \
	wget -q $(ALPINE_URL) -O - | tar xzf - boot/vmlinuz-virt --strip-components 1

$(GUEST_BIN): $(GUEST_ASM)
	nasm -f bin -o $@ $<

clean:
	rm -f $(GUEST_BIN)
	cargo clean

lint:
	cargo fmt --check && \
	cargo clippy -- -D warnings
