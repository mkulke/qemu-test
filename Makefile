GUEST_ASM = src/boot.asm
GUEST_BIN = payload/guest.bin
VMLINUZ = payload/vmlinuz-virt
ALPINE_URL = https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/x86_64/alpine-netboot-3.23.3-x86_64.tar.gz

.PHONY: build run clean lint

build: $(GUEST_BIN) $(VMLINUZ)
	cargo build

run: build
	cargo run

$(VMLINUZ):
	cd payload && \
	wget -q $(ALPINE_URL) -O - | tar xzf - boot/vmlinuz-virt --strip-components 1

$(GUEST_BIN): $(GUEST_ASM)
	nasm -f bin -o $@ $<

clean:
	rm -f $(GUEST_BIN)
	cargo clean

lint:
	cargo fmt --check
	cargo clippy -- -D warnings
