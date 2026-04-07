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
REQUIRED_TOOLS = cargo nasm wget $(QEMU_BIN) ssh-keygen mkdosfs mcopy gcc cpio gzip

.PHONY: build run clean lint check-tools

check-tools:
	@$(foreach tool,$(REQUIRED_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

build: check-tools $(GUEST_BIN) $(GUEST_PIO_STR_BIN) $(VMLINUZ) $(INITRD) $(OS_IMAGE) $(OVMF_CODE)
	cargo build

run: build
	cargo run

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

$(INITRD): $(INIT_BIN)
	d=$$(mktemp -d) && \
	mkdir -p $$d/{dev,proc,sys} && \
	cp $< $$d/init && \
	(cd $$d && find . | cpio --quiet -o -H newc | gzip -9) > $@ && \
	rm -rf $$d

clean:
	rm -f $(GUEST_BIN) $(GUEST_PIO_STR_BIN) $(INIT_BIN) $(INITRD)
	cargo clean

lint:
	cargo fmt --check && \
	cargo clippy -- -D warnings
