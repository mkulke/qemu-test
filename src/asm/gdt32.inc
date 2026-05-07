; Flat 32-bit GDT: null + code (0x08) + data (0x10) segments.

%macro GDT32 0
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
%endmacro
