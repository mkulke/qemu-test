; 16-bit real mode → 32-bit protected mode transition.
; Requires GDT32 to be invoked elsewhere in the file (defines gdtdesc).

%macro ENTER_PMODE32 0
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
%endmacro
