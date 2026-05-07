; Minimal x86 boot sector that periodically writes a message to COM1 (serial port).
; Switches to 32-bit protected mode (like QEMU's own migration test bootblock)
; to avoid real-mode PIT/PIC compatibility issues across QEMU versions.
; Assemble with: nasm -I src/asm -f bin -o guest.bin boot.asm

%include "pm32.inc"
%include "gdt32.inc"
%include "serial32.inc"

ENTER_PMODE32

main_loop:
    mov esi, message
    call serial_out_string

    ; Simple counter-based delay (~1s at typical QEMU speed)
    mov ecx, 0x3000000
.spin:
    dec ecx
    jnz .spin
    jmp main_loop

SERIAL32

message: db "HELLO FROM GUEST", 13, 10, 0

GDT32

; Pad to 510 bytes and add boot signature
times 510 - ($ - $$) db 0
dw 0xaa55

; Pad to 8KB so QEMU accepts this as a valid disk image
times 8192 - ($ - $$) db 0
