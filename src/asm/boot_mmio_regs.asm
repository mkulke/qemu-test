; Boot stub for the C register-exercising MMIO test.
;
; 16-bit real mode → load sectors from disk → 32-bit protected mode →
; call c_main() in linked C code.
;
; BIOS only loads the first 512-byte sector (the boot sector) to 0x7C00.
; The C code lives in subsequent sectors, so we must use INT 13h to read
; them into memory at 0x7E00 before switching to protected mode.
;
; Build (see Makefile):
;   nasm -I src/asm -f elf32 -o boot_stub.o boot_mmio_regs.asm
;   gcc -m32 -ffreestanding -c -o mmio_regs.o mmio_regs.c
;   ld -m elf_i386 -T mmio_regs.ld -o guest_mmio_regs.bin boot_stub.o mmio_regs.o

%include "gdt32.inc"

[bits 16]
section .boot
global _start
extern c_main

_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00                  ; stack below boot sector

    ; ── Load remaining sectors from disk into memory at 0x7E00 ──
    ; Read 15 sectors (512*15 = 7680 bytes) starting at sector 2 (LBA 1)
    ; to 0x0000:0x7E00.  This covers the C code + data.
    mov ah, 0x02                    ; BIOS read sectors
    mov al, 15                      ; number of sectors
    mov ch, 0                       ; cylinder 0
    mov cl, 2                       ; start at sector 2 (1-based)
    mov dh, 0                       ; head 0
    ; DL = drive number, preserved from BIOS boot
    mov bx, 0x7E00                  ; ES:BX = destination
    int 0x13
    jc halt16                       ; on error, halt

    ; ── Switch to 32-bit protected mode ──
    lgdt [gdtdesc]
    mov eax, 1
    mov cr0, eax
    jmp 0x08:start32

halt16:
    hlt
    jmp halt16

[bits 32]
start32:
    mov ax, 0x10                    ; flat data segment
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov esp, 0x10000                ; 64KB stack

    call c_main

    hlt
    jmp $ - 1

; ──────────────────────────────────────────────
; GDT — flat 32-bit code + data segments
; ──────────────────────────────────────────────
GDT32
