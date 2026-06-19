; 16-bit real mode -> 64-bit long mode transition.
; Loads sectors 1-15 into memory at 0x7e00, builds a 2MB identity map,
; enters long mode, and jumps to start64 with flat 64-bit segments loaded.

%macro ENTER_LONG_MODE64 0
[bits 16]
[org 0x7c00]

start:
    cli
    ; Enable A20 via fast A20 gate.
    in al, 0x92
    or al, 2
    and al, ~1
    out 0x92, al

    ; Load sectors 1-15 (7.5KB after boot sector) into memory at 0x7E00.
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov bx, 0x7E00
    mov ah, 0x02
    mov al, 15
    mov ch, 0
    mov cl, 2
    mov dh, 0
    mov dl, 0x80
    int 0x13
    jc halt

    lgdt [gdtdesc]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    jmp 0x08:start32

halt:
    hlt
    jmp halt

align 4
gdt:
    dq 0
    dw 0xFFFF, 0
    db 0, 0x9A, 0xCF, 0       ; 32-bit code
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0       ; 32-bit data
    dw 0xFFFF, 0
    db 0, 0x9A, 0xAF, 0       ; 64-bit code
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0       ; 64-bit data
gdtdesc:
    dw gdtdesc - gdt - 1
    dd gdt

[bits 32]
start32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov esp, 0x7c00

    ; Identity-map the first 2MB with PML4/PDPT/PD at 0x1000/0x2000/0x3000.
    mov edi, 0x1000
    xor eax, eax
    mov ecx, 1024
    cld
    rep stosd
    mov edi, 0x2000
    xor eax, eax
    mov ecx, 1024
    rep stosd
    mov edi, 0x3000
    xor eax, eax
    mov ecx, 1024
    rep stosd
    mov dword [0x1000], 0x2000 | 0x03
    mov dword [0x2000], 0x3000 | 0x03
    mov dword [0x3000], 0x00 | 0x83

    mov eax, cr4
    or eax, (1 << 5)           ; PAE
    mov cr4, eax

    mov eax, 0x1000
    mov cr3, eax

    mov ecx, 0xC0000080        ; IA32_EFER
    rdmsr
    or eax, (1 << 8)           ; LME
    wrmsr

    mov eax, cr0
    or eax, (1 << 31)          ; PG
    mov cr0, eax

    jmp 0x18:start64

times 510 - ($ - $$) db 0
dw 0xaa55

[bits 64]
start64:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov rsp, 0x7000
%endmacro
