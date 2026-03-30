; Minimal x86 boot sector that periodically writes a message to COM1 (serial port).
; Switches to 32-bit protected mode (like QEMU's own migration test bootblock)
; to avoid real-mode PIT/PIC compatibility issues across QEMU versions.
; Assemble with: nasm -f bin -o guest.bin boot.asm

[bits 16]
[org 0x7c00]

start:
    cli
    lgdt [gdtdesc]
    mov eax, 1
    mov cr0, eax            ; enable protected mode
    jmp 0x08:start32        ; far jump to 32-bit code segment

[bits 32]
start32:
    mov ax, 0x10            ; data segment selector
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov esp, 0x7c00

main_loop:
    mov esi, message

.print_loop:
    lodsb
    test al, al
    jz .delay

.wait_tx:
    mov dx, 0x3fd           ; Line Status Register
    in al, dx
    test al, 0x20           ; TX holding register empty?
    jz .wait_tx

    mov al, [esi - 1]       ; reload char (al was clobbered by LSR read)
    mov dx, 0x3f8
    out dx, al
    jmp .print_loop

.delay:
    ; Simple counter-based delay (~1s at typical QEMU speed)
    mov ecx, 0x3000000
.spin:
    dec ecx
    jnz .spin
    jmp main_loop

message: db "HELLO FROM GUEST", 13, 10, 0

; GDT with null, code, and data segments
align 4
gdt:
    dq 0                    ; null descriptor
    ; code segment: base=0, limit=4GB, 32-bit, execute/read, DPL=0
    dw 0xFFFF, 0
    db 0, 0x9A, 0xCF, 0
    ; data segment: base=0, limit=4GB, 32-bit, read/write, DPL=0
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0

gdtdesc:
    dw gdtdesc - gdt - 1    ; limit
    dd gdt                  ; base address

; Pad to 510 bytes and add boot signature
times 510 - ($ - $$) db 0
dw 0xaa55

; Pad to 8KB so QEMU accepts this as a valid disk image
times 8192 - ($ - $$) db 0
