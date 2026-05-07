; Minimal x86 boot sector that enters 64-bit long mode, loads known patterns
; into all 16 YMM registers, then continuously verifies they are intact.
; Designed to test XSAVE state preservation across live migration.
;
; Serial output milestones:
;   LM:OK        - long mode entered successfully
;   AVX:OK       - AVX enabled via XSETBV
;   YMM:LOADED   - all 16 YMM registers loaded with known patterns
;   AVX2:READY   - ready for migration
;   AVX2:OK      - YMM registers verified intact (repeats in loop)
;   AVX2:FAIL    - YMM register corruption detected
;   AVX2:UNSUPPORTED - host CPU lacks required features
;
; Assemble with: nasm -f bin -o guest_avx2.bin boot_avx2.asm

; ============================================================
; 16-bit real mode entry
; ============================================================
[bits 16]
[org 0x7c00]

start:
    cli
    ; Enable A20 via fast A20 gate
    in al, 0x92
    or al, 2
    and al, ~1
    out 0x92, al

    ; Load sectors 1-15 (7.5KB after boot sector) into memory at 0x7E00
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov bx, 0x7E00            ; destination buffer
    mov ah, 0x02               ; BIOS read sectors
    mov al, 15                 ; number of sectors to read
    mov ch, 0                  ; cylinder 0
    mov cl, 2                  ; start at sector 2 (1-based)
    mov dh, 0                  ; head 0
    mov dl, 0x80               ; first hard disk
    int 0x13
    jc .unsupported            ; if read fails, bail out

    ; --- CPUID feature checks ---
    mov eax, 1
    cpuid
    test ecx, (1 << 26)        ; XSAVE
    jz .unsupported
    test ecx, (1 << 28)        ; AVX
    jz .unsupported
    mov eax, 7
    xor ecx, ecx
    cpuid
    test ebx, (1 << 5)         ; AVX2
    jz .unsupported
    jmp .features_ok

.unsupported:
    mov si, .unsup_msg
.unsup_loop:
    lodsb
    test al, al
    jz .halt
    mov dx, 0x3f8
    out dx, al
    jmp .unsup_loop
.halt:
    hlt
    jmp .halt
.unsup_msg: db "AVX2:UNSUPPORTED", 13, 10, 0

.features_ok:
    lgdt [gdtdesc]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    jmp 0x08:start32

; GDT: null, 32-bit code (0x08), 32-bit data (0x10), 64-bit code (0x18), 64-bit data (0x20)
align 4
gdt:
    dq 0
    dw 0xFFFF, 0
    db 0, 0x9A, 0xCF, 0       ; 32-bit code
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0       ; 32-bit data
    dw 0xFFFF, 0
    db 0, 0x9A, 0xAF, 0       ; 64-bit code (L=1, D=0)
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0       ; 64-bit data
gdtdesc:
    dw gdtdesc - gdt - 1
    dd gdt

; ============================================================
; 32-bit protected mode: set up paging and enter long mode
; ============================================================
[bits 32]
start32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov esp, 0x7c00

    ; Build identity-mapped page tables (2MB page)
    ; PML4 at 0x1000, PDPT at 0x2000, PD at 0x3000
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
    mov dword [0x3000], 0x00 | 0x83    ; 2MB page, present + writable + PS

    ; Enable PAE
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax

    ; Load PML4
    mov eax, 0x1000
    mov cr3, eax

    ; Enable long mode (IA32_EFER.LME)
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8)
    wrmsr

    ; Enable paging
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax

    ; Far jump to 64-bit code
    jmp 0x18:start64

; Pad to 510 bytes and add boot signature
times 510 - ($ - $$) db 0
dw 0xaa55

; ============================================================
; 64-bit long mode (beyond boot sector, loaded as part of disk image)
; ============================================================
[bits 64]
start64:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov rsp, 0x7c00

    ; Print "LM:OK"
    lea rsi, [rel msg_lm_ok]
    call serial_print

    ; Enable AVX: clear CR0.EM, set CR0.MP, clear CR0.TS
    mov rax, cr0
    and al, ~(1 << 2)
    or al, (1 << 1)
    and rax, ~(1 << 3)
    mov cr0, rax

    ; Set CR4.OSFXSR | CR4.OSXMMEXCPT | CR4.OSXSAVE
    mov rax, cr4
    or eax, (1 << 9) | (1 << 10) | (1 << 18)
    mov cr4, rax

    ; XSETBV: enable x87 + SSE + AVX in XCR0
    xor ecx, ecx
    mov eax, 0x07
    xor edx, edx
    xsetbv

    lea rsi, [rel msg_avx_ok]
    call serial_print

    ; Load known patterns into YMM0-YMM15
    lea rsi, [rel ymm_patterns]
    vmovdqu ymm0,  [rsi + 0*32]
    vmovdqu ymm1,  [rsi + 1*32]
    vmovdqu ymm2,  [rsi + 2*32]
    vmovdqu ymm3,  [rsi + 3*32]
    vmovdqu ymm4,  [rsi + 4*32]
    vmovdqu ymm5,  [rsi + 5*32]
    vmovdqu ymm6,  [rsi + 6*32]
    vmovdqu ymm7,  [rsi + 7*32]
    vmovdqu ymm8,  [rsi + 8*32]
    vmovdqu ymm9,  [rsi + 9*32]
    vmovdqu ymm10, [rsi + 10*32]
    vmovdqu ymm11, [rsi + 11*32]
    vmovdqu ymm12, [rsi + 12*32]
    vmovdqu ymm13, [rsi + 13*32]
    vmovdqu ymm14, [rsi + 14*32]
    vmovdqu ymm15, [rsi + 15*32]

    lea rsi, [rel msg_ymm_loaded]
    call serial_print

    lea rsi, [rel msg_ready]
    call serial_print

; ============================================================
; Verification loop: spill YMM regs to memory, compare with GPRs
; ============================================================
verify_loop:
    ; Spill all YMM registers to buffer at 0x5000
    mov rdi, 0x5000
    vmovdqu [rdi + 0*32],  ymm0
    vmovdqu [rdi + 1*32],  ymm1
    vmovdqu [rdi + 2*32],  ymm2
    vmovdqu [rdi + 3*32],  ymm3
    vmovdqu [rdi + 4*32],  ymm4
    vmovdqu [rdi + 5*32],  ymm5
    vmovdqu [rdi + 6*32],  ymm6
    vmovdqu [rdi + 7*32],  ymm7
    vmovdqu [rdi + 8*32],  ymm8
    vmovdqu [rdi + 9*32],  ymm9
    vmovdqu [rdi + 10*32], ymm10
    vmovdqu [rdi + 11*32], ymm11
    vmovdqu [rdi + 12*32], ymm12
    vmovdqu [rdi + 13*32], ymm13
    vmovdqu [rdi + 14*32], ymm14
    vmovdqu [rdi + 15*32], ymm15

    ; Compare against expected patterns using GPR qword loads
    lea rsi, [rel ymm_patterns]
    mov rdi, 0x5000
    mov rcx, 64                ; 16 regs * 32 bytes / 8 = 64 qwords
.cmp_loop:
    mov rax, [rsi]
    cmp rax, [rdi]
    jne .fail
    add rsi, 8
    add rdi, 8
    dec rcx
    jnz .cmp_loop

    lea rsi, [rel msg_ok]
    call serial_print
    jmp .delay

.fail:
    lea rsi, [rel msg_fail]
    call serial_print

.delay:
    mov rcx, 0x3000000
.spin:
    dec rcx
    jnz .spin
    jmp verify_loop

; ============================================================
; serial_print: print NUL-terminated string at RSI to COM1
; ============================================================
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

; ============================================================
; Data
; ============================================================
msg_lm_ok:       db "LM:OK", 13, 10, 0
msg_avx_ok:      db "AVX:OK", 13, 10, 0
msg_ymm_loaded:  db "YMM:LOADED", 13, 10, 0
msg_ready:       db "AVX2:READY", 13, 10, 0
msg_ok:          db "AVX2:OK", 13, 10, 0
msg_fail:        db "AVX2:FAIL", 13, 10, 0

; 16 distinct 32-byte patterns for YMM0-YMM15.
; Upper and lower 128-bit halves intentionally differ to detect
; partial XSAVE that only preserves the XMM (lower) portion.
align 32
ymm_patterns:
    dd 0x10101010, 0x10101010, 0x10101010, 0x10101010
    dd 0x80808080, 0x80808080, 0x80808080, 0x80808080
    dd 0x21212121, 0x21212121, 0x21212121, 0x21212121
    dd 0x91919191, 0x91919191, 0x91919191, 0x91919191
    dd 0x32323232, 0x32323232, 0x32323232, 0x32323232
    dd 0xA2A2A2A2, 0xA2A2A2A2, 0xA2A2A2A2, 0xA2A2A2A2
    dd 0x43434343, 0x43434343, 0x43434343, 0x43434343
    dd 0xB3B3B3B3, 0xB3B3B3B3, 0xB3B3B3B3, 0xB3B3B3B3
    dd 0x54545454, 0x54545454, 0x54545454, 0x54545454
    dd 0xC4C4C4C4, 0xC4C4C4C4, 0xC4C4C4C4, 0xC4C4C4C4
    dd 0x65656565, 0x65656565, 0x65656565, 0x65656565
    dd 0xD5D5D5D5, 0xD5D5D5D5, 0xD5D5D5D5, 0xD5D5D5D5
    dd 0x76767676, 0x76767676, 0x76767676, 0x76767676
    dd 0xE6E6E6E6, 0xE6E6E6E6, 0xE6E6E6E6, 0xE6E6E6E6
    dd 0x87878787, 0x87878787, 0x87878787, 0x87878787
    dd 0xF7F7F7F7, 0xF7F7F7F7, 0xF7F7F7F7, 0xF7F7F7F7
    dd 0x18181818, 0x18181818, 0x18181818, 0x18181818
    dd 0x88888888, 0x88888888, 0x88888888, 0x88888888
    dd 0x29292929, 0x29292929, 0x29292929, 0x29292929
    dd 0x99999999, 0x99999999, 0x99999999, 0x99999999
    dd 0x3A3A3A3A, 0x3A3A3A3A, 0x3A3A3A3A, 0x3A3A3A3A
    dd 0xAAAAAAAA, 0xAAAAAAAA, 0xAAAAAAAA, 0xAAAAAAAA
    dd 0x4B4B4B4B, 0x4B4B4B4B, 0x4B4B4B4B, 0x4B4B4B4B
    dd 0xBBBBBBBB, 0xBBBBBBBB, 0xBBBBBBBB, 0xBBBBBBBB
    dd 0x5C5C5C5C, 0x5C5C5C5C, 0x5C5C5C5C, 0x5C5C5C5C
    dd 0xCCCCCCCC, 0xCCCCCCCC, 0xCCCCCCCC, 0xCCCCCCCC
    dd 0x6D6D6D6D, 0x6D6D6D6D, 0x6D6D6D6D, 0x6D6D6D6D
    dd 0xDDDDDDDD, 0xDDDDDDDD, 0xDDDDDDDD, 0xDDDDDDDD
    dd 0x7E7E7E7E, 0x7E7E7E7E, 0x7E7E7E7E, 0x7E7E7E7E
    dd 0xEEEEEEEE, 0xEEEEEEEE, 0xEEEEEEEE, 0xEEEEEEEE
    dd 0x8F8F8F8F, 0x8F8F8F8F, 0x8F8F8F8F, 0x8F8F8F8F
    dd 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF

; Pad to 8KB so QEMU accepts this as a valid disk image
times 8192 - ($ - $$) db 0
