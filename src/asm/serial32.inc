; Serial output routines for 32-bit protected mode (COM1).

COM1_DATA equ 0x3F8
COM1_LSR  equ 0x3FD

%macro SERIAL32 0
; serial_out — send byte in AL to COM1.
;   Waits for TX-ready, preserves all regs except EFLAGS.
serial_out:
    push eax
    push edx
.wait:
    mov dx, COM1_LSR
    in al, dx
    test al, 0x20           ; TX holding register empty?
    jz .wait
    pop edx
    pop eax
    push edx
    mov dx, COM1_DATA
    out dx, al
    pop edx
    ret

; serial_out_string — send NUL-terminated string at ESI.
;   Preserves ESI.
serial_out_string:
    push esi
.next:
    lodsb
    test al, al
    jz .done
    call serial_out
    jmp .next
.done:
    pop esi
    ret
%endmacro
