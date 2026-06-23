GUEST_ASM = src/asm/boot.asm
GUEST_BIN = payload/guest.bin
GUEST_PIO_STR_ASM = src/asm/boot_pio_str.asm
GUEST_PIO_STR_BIN = payload/guest_pio_str.bin
GUEST_AVX2_ASM = src/asm/boot_avx2.asm
GUEST_AVX2_BIN = payload/guest_avx2.bin
GUEST_SCALAR_ASM = src/asm/boot_scalar.asm
GUEST_SCALAR_BIN = payload/guest_scalar.bin
GUEST_FP_SSE_ASM = src/asm/boot_fp_sse.asm
GUEST_FP_SSE_BIN = payload/guest_fp_sse.bin
GUEST_MMIO_ASM = src/asm/boot_mmio.asm
GUEST_MMIO_BIN = payload/guest_mmio.bin
GUEST_PIO_VMPORT_ASM = src/asm/boot_pio_vmport.asm
GUEST_PIO_VMPORT_BIN = payload/guest_pio_vmport.bin
GUEST_MMIO_REGS_ASM = src/asm/boot_mmio_regs.asm
GUEST_MMIO_REGS_C = src/mmio_regs.c
GUEST_MMIO_REGS_LD = src/mmio_regs.ld
GUEST_MMIO_REGS_BIN = payload/guest_mmio_regs.bin
ASM_INCLUDES = $(wildcard src/asm/*.inc)
VMLINUZ = payload/vmlinuz-virt
INITRD = payload/initrd.img
INIT_BIN = payload/init
INIT_SRC = src/lm_init.c
OS_IMAGE = payload/os-image.qcow2
OVMF_CODE = payload/OVMF_CODE.fd
ALPINE_URL = https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/x86_64/alpine-netboot-3.23.3-x86_64.tar.gz
OVMF_DEB_URL = http://security.debian.org/debian-security/pool/updates/main/e/edk2/ovmf_2022.11-6+deb12u1_all.deb
MKOSI_SHA = 9a28ad20bbea61894ea7b971d318a71f4374cf3b
MKOSI_CMD = uv tool run --from git+https://github.com/systemd/mkosi.git#$(MKOSI_SHA)
MKOSI_CONF = src/os-image/mkosi.conf
IMAGE_RAW = payload/image.raw
STRESS_NG_UNIT = src/os-image/mkosi.extra/etc/systemd/system/stress-ng.service
QEMU_BIN ?= qemu-system-x86_64
REQUIRED_BUILD_TOOLS = cargo nasm wget gcc cpio gzip qemu-img dnf
REQUIRED_TOOLS = $(QEMU_BIN) ssh-keygen mkdosfs mcopy xmlstarlet
BRIDGE_NAME = qemu-br0
BRIDGE_ADDR = 192.168.100.1/24
TAP_PREFIX = tap-qemu
NUM_TAPS ?= 2
RELEASE_BIN = target/release/qemu-test
RUST_SOURCES := $(shell find src -name "*.rs") build.rs Cargo.toml Cargo.lock
EMBEDDED_PAYLOADS = $(GUEST_BIN) \
		   $(GUEST_PIO_STR_BIN) \
		   $(GUEST_PIO_VMPORT_BIN) \
		   $(GUEST_AVX2_BIN) \
		   $(GUEST_SCALAR_BIN) \
		   $(GUEST_FP_SSE_BIN) \
		   $(GUEST_MMIO_BIN) \
		   $(GUEST_MMIO_REGS_BIN)
RUNTIME_PAYLOADS = $(VMLINUZ) \
		   $(INITRD) \
		   $(OS_IMAGE) \
		   $(OVMF_CODE)

.PHONY: echo-runtime-payloads build build-payloads build-release run run-release clean fmt lint check-build-tools check-tools setup-bridge teardown-bridge

echo-cachable-payloads:
	@$(foreach p,$(RUNTIME_PAYLOADS),realpath $(p);)

check-tools:
	@$(foreach tool,$(REQUIRED_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

check-build-tools:
	@$(foreach tool,$(REQUIRED_BUILD_TOOLS),command -v $(tool) >/dev/null 2>&1 || { echo "error: $(tool) not found"; exit 1; };)

build-payloads: check-build-tools $(EMBEDDED_PAYLOADS) $(RUNTIME_PAYLOADS)

build: build-payloads
	cargo build
	cargo test

run: build check-tools
	cargo run

$(RELEASE_BIN): $(RUST_SOURCES) $(EMBEDDED_PAYLOADS)
	cargo build --release --locked && \
	cargo test --release --locked

build-release: $(RELEASE_BIN) $(RUNTIME_PAYLOADS)

run-release: $(RELEASE_BIN) check-tools
	./$(RELEASE_BIN)

$(OVMF_CODE):
	cd payload && \
	wget -q $(OVMF_DEB_URL) -O ovmf.deb && \
	ar p ovmf.deb data.tar.xz | tar xJ --strip-components=4 ./usr/share/OVMF/OVMF_CODE.fd && \
	rm ovmf.deb

$(VMLINUZ):
	cd payload && \
	wget -q $(ALPINE_URL) -O - | tar xzf - boot/vmlinuz-virt --strip-components 1

$(GUEST_BIN): $(GUEST_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_PIO_STR_BIN): $(GUEST_PIO_STR_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_AVX2_BIN): $(GUEST_AVX2_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_SCALAR_BIN): $(GUEST_SCALAR_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_FP_SSE_BIN): $(GUEST_FP_SSE_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_MMIO_BIN): $(GUEST_MMIO_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_PIO_VMPORT_BIN): $(GUEST_PIO_VMPORT_ASM) $(ASM_INCLUDES)
	nasm -I src/asm/ -f bin -o $@ $<

$(GUEST_MMIO_REGS_BIN): $(GUEST_MMIO_REGS_ASM) $(GUEST_MMIO_REGS_C) $(GUEST_MMIO_REGS_LD) $(ASM_INCLUDES)
	nasm -I src/asm/ -f elf32 -o payload/boot_stub_regs.o $(GUEST_MMIO_REGS_ASM)
	gcc -m32 -ffreestanding -fno-pie -fno-stack-protector -fomit-frame-pointer -masm=intel -c -o payload/mmio_regs.o $(GUEST_MMIO_REGS_C)
	ld -m elf_i386 -T $(GUEST_MMIO_REGS_LD) -o $@ payload/boot_stub_regs.o payload/mmio_regs.o
	truncate -s 8192 $@
	rm -f payload/boot_stub_regs.o payload/mmio_regs.o

$(INIT_BIN): $(INIT_SRC)
	gcc -static -o $@ $<

.DELETE_ON_ERROR:
$(INITRD): $(INIT_BIN)
	d=$$(mktemp -d) && \
	mkdir -p $$d/{dev,proc,sys} && \
	cp $< $$d/init && \
	(cd $$d && find . | cpio --quiet -o -H newc | gzip -9) > $@ && \
	rm -rf $$d

$(IMAGE_RAW): $(MKOSI_CONF) $(STRESS_NG_UNIT)
	$(MKOSI_CMD) -- mkosi build --force --directory src/os-image

.DELETE_ON_ERROR:
$(OS_IMAGE): $(IMAGE_RAW)
	qemu-img convert -f raw -O qcow2 $< $@

clean:
	rm -f $(EMBEDDED_PAYLOADS) $(RUNTIME_PAYLOADS)
	$(MKOSI_CMD) -- mkosi clean -ff --directory src/os-image
	cargo clean

fmt:
	cargo fmt

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
