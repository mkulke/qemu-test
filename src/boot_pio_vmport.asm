; Bare-metal test for PIO with cpu_synchronize_state interaction (vmport).
;
; VMPort (I/O port 0x5658) calls cpu_synchronize_state() during a port read,
; which pulls all vCPU registers into QEMU's internal state and sets the
; "dirty" flag.  The vmport command handler then reads and writes GPRs on
; the QEMU-side state directly (e.g. setting EBX = VMPORT_MAGIC for
; CMD_GETVERSION).
;
; The bug under test:
;   After vmport returns, the PIO fast-path handler writes only RIP and RAX
;   back to the hypervisor via set_x64_registers(), then clears the dirty
;   flag.  This means GPR changes made by vmport (EBX, ECX, etc.) are never
;   flushed to the hypervisor — the guest sees stale values.
;
; The fix:
;   When dirty is already true after pio_read (because cpu_synchronize_state
;   was called), the PIO handler must update the QEMU-side state instead of
;   writing directly to the hypervisor, letting the normal dirty-flush path
;   propagate all register changes on the next vCPU entry.
;
; Tests:
;   A = CMD_GETVERSION (10): EBX must change to VMPORT_MAGIC
;   B = CMD_GETRAMSIZE (20): EBX must change to 0x1177
;   C = CMD_GETVERSION preserves other GPRs (ESI, EDI, EBP)
;
; Expected serial output on success: "ABC VMPORT_OK\r\n"
; A lowercase letter indicates which test failed.
;
; Assemble: nasm -f bin -o guest_pio_vmport.bin boot_pio_vmport.asm

VMPORT_MAGIC    equ 0x564D5868
VMPORT_PORT     equ 0x5658
CMD_GETVERSION  equ 10
CMD_GETRAMSIZE  equ 20
RAMSIZE_EBX     equ 0x1177

SERIAL_DATA     equ 0x3F8
SERIAL_LSR      equ 0x3FD

MAGIC_ESI       equ 0xABCD1234
MAGIC_EDI       equ 0x87654321
MAGIC_EBP       equ 0xFEEDFACE

; ──────────────────────────────────────────────
; 16-bit real-mode entry
; ──────────────────────────────────────────────
[bits 16]
[org 0x7c00]

start:
    cli
    lgdt [gdtdesc]
    mov eax, 1
    mov cr0, eax
    jmp 0x08:start32

; ──────────────────────────────────────────────
; 32-bit protected-mode code
; ──────────────────────────────────────────────
[bits 32]
start32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov esp, 0x7c00

    ; ==================================================================
    ; Test A: CMD_GETVERSION — EBX must be set to VMPORT_MAGIC
    ; ==================================================================
    ; vmport_cmd_get_version() does:
    ;   cpu->env.regs[R_EBX] = VMPORT_MAGIC;
    ; If the PIO handler clears dirty without flushing, EBX keeps its
    ; old value and this test fails.
    mov ebx, 0xDEADBEEF             ; known non-magic value
    mov eax, VMPORT_MAGIC
    mov ecx, CMD_GETVERSION
    mov edx, VMPORT_PORT
    in eax, dx                      ; triggers vmport

    cmp ebx, VMPORT_MAGIC
    je .ta_pass
    mov al, 'a'
    jmp .ta_out
.ta_pass:
    mov al, 'A'
.ta_out:
    call serial_out

    ; ==================================================================
    ; Test B: CMD_GETRAMSIZE — EBX must be set to 0x1177
    ; ==================================================================
    ; vmport_cmd_ram_size() does:
    ;   cpu->env.regs[R_EBX] = 0x1177;
    ; Same dirty-flag bug as Test A, different command and expected value.
    mov ebx, 0xCAFEBABE             ; known different value
    mov eax, VMPORT_MAGIC
    mov ecx, CMD_GETRAMSIZE
    mov edx, VMPORT_PORT
    in eax, dx

    cmp ebx, RAMSIZE_EBX
    je .tb_pass
    mov al, 'b'
    jmp .tb_out
.tb_pass:
    mov al, 'B'
.tb_out:
    call serial_out

    ; ==================================================================
    ; Test C: CMD_GETVERSION preserves non-vmport GPRs
    ; ==================================================================
    ; cpu_synchronize_state loads all regs from the hypervisor, then
    ; dirty=true should cause them to be flushed back.  Set ESI, EDI,
    ; EBP to known values, invoke vmport (which only modifies EBX),
    ; then verify the others survived the round-trip.
    mov esi, MAGIC_ESI
    mov edi, MAGIC_EDI
    mov ebp, MAGIC_EBP
    mov ebx, 0
    mov eax, VMPORT_MAGIC
    mov ecx, CMD_GETVERSION
    mov edx, VMPORT_PORT
    in eax, dx

    cmp esi, MAGIC_ESI
    jne .tc_fail
    cmp edi, MAGIC_EDI
    jne .tc_fail
    cmp ebp, MAGIC_EBP
    jne .tc_fail
    mov al, 'C'
    jmp .tc_out
.tc_fail:
    mov al, 'c'
.tc_out:
    call serial_out

    ; ==================================================================
    ; All tests done — emit result marker
    ; ==================================================================
    mov al, ' '
    call serial_out

    mov esi, ok_msg
    call serial_out_string

    hlt
    jmp $ - 1

; ──────────────────────────────────────────────
; serial_out — send byte in AL to COM1
;   Waits for TX-ready, preserves all regs except EFLAGS
; ──────────────────────────────────────────────
serial_out:
    push eax
    push edx
.wait:
    mov dx, SERIAL_LSR
    in al, dx
    test al, 0x20
    jz .wait
    pop edx
    pop eax
    push edx
    mov dx, SERIAL_DATA
    out dx, al
    pop edx
    ret

; ──────────────────────────────────────────────
; serial_out_string — send NUL-terminated string at ESI
;   Preserves ESI
; ──────────────────────────────────────────────
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

; ──────────────────────────────────────────────
; Data
; ──────────────────────────────────────────────
ok_msg: db "VMPORT_OK", 13, 10, 0

; ──────────────────────────────────────────────
; GDT — flat 32-bit code + data segments
; ──────────────────────────────────────────────
align 4
gdt:
    dq 0
    dw 0xFFFF, 0
    db 0, 0x9A, 0xCF, 0
    dw 0xFFFF, 0
    db 0, 0x92, 0xCF, 0
gdtdesc:
    dw gdtdesc - gdt - 1
    dd gdt

; ──────────────────────────────────────────────
; Boot signature + padding
; ──────────────────────────────────────────────
times 510 - ($ - $$) db 0
dw 0xAA55
