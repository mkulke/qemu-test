; Minimal x86_64 boot payload for legacy FP/SSE migration checks.
;
; Serial output milestones:
;   FPSSE:READY - long mode entered; ready for migration
;   FPSSE:OK    - x87/MXCSR/XMM state verified (repeats)
;   FPSSE:FAIL_* - FP/SSE state corruption diagnostic

%include "lm64.inc"
%include "serial64.inc"

ENTER_LONG_MODE64

    mov rax, cr0
    and rax, ~(1 << 2)         ; clear EM
    or rax, (1 << 1)           ; set MP
    and rax, ~(1 << 3)         ; clear TS
    mov cr0, rax

    mov rax, cr4
    or rax, (1 << 9) | (1 << 10)
    mov cr4, rax

    lea rsi, [rel msg_ready]
    call serial_print

state_loop:
    finit
    fldcw [rel x87_cw_value]
    ldmxcsr [rel mxcsr_value]

    lea rsi, [rel xmm_patterns]
    movdqu xmm0,  [rsi + 0*16]
    movdqu xmm1,  [rsi + 1*16]
    movdqu xmm2,  [rsi + 2*16]
    movdqu xmm3,  [rsi + 3*16]
    movdqu xmm4,  [rsi + 4*16]
    movdqu xmm5,  [rsi + 5*16]
    movdqu xmm6,  [rsi + 6*16]
    movdqu xmm7,  [rsi + 7*16]
    movdqu xmm8,  [rsi + 8*16]
    movdqu xmm9,  [rsi + 9*16]
    movdqu xmm10, [rsi + 10*16]
    movdqu xmm11, [rsi + 11*16]
    movdqu xmm12, [rsi + 12*16]
    movdqu xmm13, [rsi + 13*16]
    movdqu xmm14, [rsi + 14*16]
    movdqu xmm15, [rsi + 15*16]

    fild qword [rel x87_values + 0*8]
    fild qword [rel x87_values + 1*8]
    fild qword [rel x87_values + 2*8]
    fild qword [rel x87_values + 3*8]

    mov rcx, 0x3000000
.spin:
    dec rcx
    jnz .spin

    fnstcw [rel x87_cw_seen]
    mov ax, [rel x87_cw_seen]
    cmp ax, [rel x87_cw_value]
    jne fail_x87_cw

    stmxcsr [rel mxcsr_seen]
    mov eax, [rel mxcsr_seen]
    cmp eax, [rel mxcsr_value]
    jne fail_mxcsr

    mov rdi, 0x5000
    movdqu [rdi + 0*16],  xmm0
    movdqu [rdi + 1*16],  xmm1
    movdqu [rdi + 2*16],  xmm2
    movdqu [rdi + 3*16],  xmm3
    movdqu [rdi + 4*16],  xmm4
    movdqu [rdi + 5*16],  xmm5
    movdqu [rdi + 6*16],  xmm6
    movdqu [rdi + 7*16],  xmm7
    movdqu [rdi + 8*16],  xmm8
    movdqu [rdi + 9*16],  xmm9
    movdqu [rdi + 10*16], xmm10
    movdqu [rdi + 11*16], xmm11
    movdqu [rdi + 12*16], xmm12
    movdqu [rdi + 13*16], xmm13
    movdqu [rdi + 14*16], xmm14
    movdqu [rdi + 15*16], xmm15

    lea rsi, [rel xmm_patterns]
    mov rdi, 0x5000
    mov rcx, 32
.cmp_xmm:
    mov rax, [rsi]
    cmp rax, [rdi]
    jne fail_xmm
    add rsi, 8
    add rdi, 8
    dec rcx
    jnz .cmp_xmm

    mov rdi, 0x6000
    fistp qword [rdi + 0*8]
    fistp qword [rdi + 1*8]
    fistp qword [rdi + 2*8]
    fistp qword [rdi + 3*8]

    mov rax, [rel x87_values + 3*8]
    cmp [rdi + 0*8], rax
    jne fail_x87_data
    mov rax, [rel x87_values + 2*8]
    cmp [rdi + 1*8], rax
    jne fail_x87_data
    mov rax, [rel x87_values + 1*8]
    cmp [rdi + 2*8], rax
    jne fail_x87_data
    mov rax, [rel x87_values + 0*8]
    cmp [rdi + 3*8], rax
    jne fail_x87_data

    lea rsi, [rel msg_ok]
    call serial_print
    jmp state_loop

fail_x87_cw:
    lea rsi, [rel msg_fail_x87_cw]
    jmp fail

fail_mxcsr:
    lea rsi, [rel msg_fail_mxcsr]
    jmp fail

fail_xmm:
    lea rsi, [rel msg_fail_xmm]
    jmp fail

fail_x87_data:
    lea rsi, [rel msg_fail_x87_data]
    jmp fail

fail:
    call serial_print
.halt:
    hlt
    jmp .halt

SERIAL64

msg_ready: db "FPSSE:READY", 13, 10, 0
msg_ok:    db "FPSSE:OK", 13, 10, 0
msg_fail_x87_cw:   db "FPSSE:FAIL_X87_CW", 13, 10, 0
msg_fail_mxcsr:    db "FPSSE:FAIL_MXCSR", 13, 10, 0
msg_fail_xmm:      db "FPSSE:FAIL_XMM", 13, 10, 0
msg_fail_x87_data: db "FPSSE:FAIL_X87_DATA", 13, 10, 0

align 16
x87_cw_value: dw 0x0f7f
x87_cw_seen:  dw 0
mxcsr_value:  dd 0x00007f80
mxcsr_seen:   dd 0

align 8
x87_values:
    dq 0x0000000011112222
    dq 0x0000000033334444
    dq 0x0000000055556666
    dq 0x0000000077778888

align 16
xmm_patterns:
    dq 0x1010101010101010, 0x8080808080808080
    dq 0x2121212121212121, 0x9191919191919191
    dq 0x3232323232323232, 0xa2a2a2a2a2a2a2a2
    dq 0x4343434343434343, 0xb3b3b3b3b3b3b3b3
    dq 0x5454545454545454, 0xc4c4c4c4c4c4c4c4
    dq 0x6565656565656565, 0xd5d5d5d5d5d5d5d5
    dq 0x7676767676767676, 0xe6e6e6e6e6e6e6e6
    dq 0x8787878787878787, 0xf7f7f7f7f7f7f7f7
    dq 0x1818181818181818, 0x8888888888888888
    dq 0x2929292929292929, 0x9999999999999999
    dq 0x3a3a3a3a3a3a3a3a, 0xaaaaaaaaaaaaaaaa
    dq 0x4b4b4b4b4b4b4b4b, 0xbbbbbbbbbbbbbbbb
    dq 0x5c5c5c5c5c5c5c5c, 0xcccccccccccccccc
    dq 0x6d6d6d6d6d6d6d6d, 0xdddddddddddddddd
    dq 0x7e7e7e7e7e7e7e7e, 0xeeeeeeeeeeeeeeee
    dq 0x8f8f8f8f8f8f8f8f, 0xffffffffffffffff

times 8192 - ($ - $$) db 0
