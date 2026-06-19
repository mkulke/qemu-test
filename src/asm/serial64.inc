; Serial output routines for 64-bit long mode (COM1).

%macro SERIAL64 0
; serial_print — send NUL-terminated string at RSI to COM1.
serial_print:
.next:
    lodsb
    test al, al
    jz .done
.wait_tx:
    mov dx, 0x3fd
    push rax
    in al, dx
    test al, 0x20
    pop rax
    jz .wait_tx
    mov dx, 0x3f8
    out dx, al
    jmp .next
.done:
    ret
%endmacro
