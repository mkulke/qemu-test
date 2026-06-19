; Minimal x86_64 boot payload for scalar CPU-state migration checks.
;
; Serial output milestones:
;   SCALAR:READY - long mode entered; ready for migration
;   SCALAR:OK    - scalar state/arithmetic verified (repeats)
;   SCALAR:FAIL  - scalar state/arithmetic corruption detected

%include "lm64.inc"
%include "serial64.inc"

ENTER_LONG_MODE64

    lea rsi, [rel msg_ready]
    call serial_print

state_loop:
    mov rbx, 0x1122334455667788
    mov rbp, 0x8877665544332211
    mov rsi, 0x0102030405060708
    mov rdi, 0x0807060504030201
    mov r8,  0x1111111122222222
    mov r9,  0x3333333344444444
    mov r10, 0x5555555566666666
    mov r11, 0x7777777788888888
    mov r12, 0x99999999aaaaaaaa
    mov r13, 0xbbbbbbbbcccccccc
    mov r14, 0xddddddddeeeeeeee
    mov r15, 0xffff00000000ffff

    mov rcx, 0x3000000
.spin:
    dec rcx
    jnz .spin

    mov rax, 0x1122334455667788
    cmp rbx, rax
    jne fail
    mov rax, 0x8877665544332211
    cmp rbp, rax
    jne fail
    mov rax, 0x0102030405060708
    cmp rsi, rax
    jne fail
    mov rax, 0x0807060504030201
    cmp rdi, rax
    jne fail
    mov rax, 0x1111111122222222
    cmp r8, rax
    jne fail
    mov rax, 0x3333333344444444
    cmp r9, rax
    jne fail
    mov rax, 0x5555555566666666
    cmp r10, rax
    jne fail
    mov rax, 0x7777777788888888
    cmp r11, rax
    jne fail
    mov rax, 0x99999999aaaaaaaa
    cmp r12, rax
    jne fail
    mov rax, 0xbbbbbbbbcccccccc
    cmp r13, rax
    jne fail
    mov rax, 0xddddddddeeeeeeee
    cmp r14, rax
    jne fail
    mov rax, 0xffff00000000ffff
    cmp r15, rax
    jne fail

    ; Division is intentionally included because stress-ng reports SIGFPE.
    mov rax, 0x123456789abcdef0
    xor rdx, rdx
    mov rcx, 0x10
    test rcx, rcx
    jz fail
    div rcx
    mov r8, 0x0123456789abcdef
    cmp rax, r8
    jne fail
    test rdx, rdx
    jne fail

    mov rax, rbx
    cmp rax, rbx
    pushfq
    pop rax
    test rax, 1                ; CF must be clear.
    jnz fail
    test rax, (1 << 6)         ; ZF must be set.
    jz fail

    mov rax, 1
    sub rax, 2
    pushfq
    pop rax
    test rax, 1                ; CF must be set.
    jz fail
    test rax, (1 << 7)         ; SF must be set.
    jz fail

    lea rsi, [rel msg_ok]
    call serial_print
    jmp state_loop

fail:
    lea rsi, [rel msg_fail]
    call serial_print
.halt:
    hlt
    jmp .halt

SERIAL64

msg_ready: db "SCALAR:READY", 13, 10, 0
msg_ok:    db "SCALAR:OK", 13, 10, 0
msg_fail:  db "SCALAR:FAIL", 13, 10, 0

times 8192 - ($ - $$) db 0
