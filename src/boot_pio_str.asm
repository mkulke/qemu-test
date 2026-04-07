; Minimal x86 boot sector that exercises the write_memory and read_memory paths
; in the MSHV accelerator with page-crossing accesses.
;
; Phase 3a: Uses REP INSD to write 4 bytes from a PCI config port into guest
; memory at an address straddling a 4KB page boundary, forcing
; handle_pio_str_read → write_memory to write across two pages.
;
; Phase 3b: Uses REP OUTSD to read 4 bytes from guest memory at an address
; straddling a different 4KB page boundary, forcing handle_pio_str_write →
; read_memory to read across two pages. The value is then read back via INSD
; into the output buffer for verification.
;
; 32-bit paging is enabled with an identity map first, then virtual pages
; 0x11000 and 0x12000 are remapped to physical 0x31000 and 0x30000 via PTE
; modification + INVLPG. This ensures both the INSD target (0x10FFD..0x11000)
; and the OUTSD source (0x11FFE..0x12001) span two non-contiguous physical
; pages. The remaps must happen after paging is enabled because MSHV builds
; shadow page tables at CR3/PG activation time and doesn't observe earlier
; PTE writes.
;
; IMPORTANT: Both write_memory and read_memory use translate_gva which may
; return identity GVA→GPA mappings. To detect failures, verification must NOT
; go through read_memory/write_memory. Serial output uses byte-by-byte
; non-string PIO (out dx, al), and buffer reads use MOV (hardware page walk).
;
; Assemble with: nasm -f bin -o guest_pio_str.bin boot_pio_str.asm

BUFFER       equ 0x10000       ; 12KB work buffer (page-aligned)
INSD_TARGET  equ 0x10FFD       ; 3 bytes before page boundary at 0x11000
OUTSD_SRC    equ 0x11FFE       ; 2 bytes before page boundary at 0x12000
READBACK_DST equ 0x11011       ; readback destination in output region
OUTPUT_START equ 0x10FF0       ; serial output begins here (13 bytes before target)
OUTPUT_END   equ 0x11015       ; serial output ends here (after readback bytes)
PCI_CFG_ADDR equ 0x0CF8        ; PCI Configuration Address register (32-bit port)
PCI_CFG_VAL  equ 0x43434444    ; 'CCDD' — all bytes printable, bits 1:0 clear
OUTSD_VAL    equ 0x59595858    ; 'XXYY' — all bytes printable, bits 1:0 clear
PAGE_DIR     equ 0x20000       ; page directory (4KB, page-aligned)
PAGE_TABLE   equ 0x21000       ; page table for first 4MB (4KB)
REMAP_PHYS   equ 0x30000       ; physical page backing virtual 0x12000
REMAP_PHYS2  equ 0x31000       ; physical page backing virtual 0x11000

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

    ; --- Set up 32-bit paging ---
    ; First enable paging with a pure identity map, then modify PTEs and INVLPG.
    ; Doing the remaps before paging is enabled doesn't work because MSHV builds
    ; shadow page tables when CR3/PG are set and doesn't observe earlier writes.

    ; Clear page directory
    mov edi, PAGE_DIR
    xor eax, eax
    mov ecx, 1024
    cld
    rep stosd

    ; Build identity-mapped page table for first 4MB
    mov edi, PAGE_TABLE
    mov eax, 0x03              ; present + read/write
    mov ecx, 1024
.fill_pt:
    mov [edi], eax
    add edi, 4
    add eax, 0x1000
    dec ecx
    jnz .fill_pt

    ; PDE[0] → page table
    mov dword [PAGE_DIR], PAGE_TABLE | 0x03

    ; Enable paging (identity-mapped)
    mov eax, PAGE_DIR
    mov cr3, eax
    mov eax, cr0
    or eax, 0x80000000
    mov cr0, eax

    ; Now remap pages and flush TLB so the hypervisor observes the changes.
    ; Remap virtual 0x11000 → physical 0x31000
    mov dword [PAGE_TABLE + 0x11 * 4], REMAP_PHYS2 | 0x03
    invlpg [0x11000]

    ; Remap virtual 0x12000 → physical 0x30000
    mov dword [PAGE_TABLE + 0x12 * 4], REMAP_PHYS | 0x03
    invlpg [0x12000]

    ; --- Phase 1: Fill 12KB buffer with 'A' (0x41) ---
    ; (Goes through paging: virtual 0x12000 writes to physical 0x30000)
    mov edi, BUFFER
    mov al, 'A'
    mov ecx, 0x3000
    cld
    rep stosb

    ; --- Phase 2: Write known value to PCI config address register ---
    mov eax, PCI_CFG_VAL
    mov dx, PCI_CFG_ADDR
    out dx, eax

    ; --- Phase 3a: Page-crossing INSD (exercises write_memory) ---
    ; REP INSD reads 4 bytes from port 0xCF8 into memory at 0x10FFD..0x11000.
    ; This triggers handle_pio_str_read → write_memory, which must split the
    ; 4-byte write across the page boundary: 3 bytes to physical 0x10FFD and
    ; 1 byte to physical 0x31000 (the remapped backing of virtual 0x11000).
    mov edi, INSD_TARGET
    mov ecx, 1              ; one 32-bit read
    mov dx, PCI_CFG_ADDR
    cld
    rep insd

    ; --- Phase 3b: Set up known data for page-crossing OUTSD ---
    ; Write OUTSD_VAL ('XXYY') at virtual 0x11FFE..0x12001. Because paging
    ; remaps virtual 0x11000 → physical 0x31000 and 0x12000 → physical 0x30000,
    ; the 'XX' bytes land at physical 0x31FFE and the 'YY' bytes land at
    ; physical 0x30000 — two non-contiguous physical pages.
    ; Bits 1:0 of the DWORD are clear so the value round-trips through 0xCF8.
    mov dword [OUTSD_SRC], OUTSD_VAL

    ; --- Phase 3c: Page-crossing OUTSD (exercises read_memory) ---
    ; REP OUTSD reads 4 bytes from memory at 0x11FFE..0x12001 and writes to
    ; port 0xCF8. This triggers handle_pio_str_write → read_memory, which must
    ; read across the page boundary at 0x12000.
    mov esi, OUTSD_SRC
    mov ecx, 1              ; one 32-bit write
    mov dx, PCI_CFG_ADDR
    cld
    rep outsd

    ; --- Phase 3d: Read back the round-tripped value ---
    ; Use non-string IN (bypasses write_memory/translate_gva) and store with
    ; MOV (hardware page walk) so the readback is independent of translate_gva.
    mov dx, PCI_CFG_ADDR
    in eax, dx
    mov dword [READBACK_DST], eax

    ; --- Phase 4: Append marker string right after the output region ---
    ; MOV/REP MOVSB uses hardware page walk, not translate_gva.
    mov esi, marker
    mov edi, OUTPUT_END
    mov ecx, marker_len
    cld
    rep movsb

main_loop:
    ; --- Serial output via non-string PIO ---
    ; Each byte is read with MOV (hardware page walk → correct GVA→GPA) and
    ; sent with OUT (non-string PIO → handle_pio_non_str, bypasses read_memory).
    ; This ensures that if write_memory/read_memory wrote to the wrong physical
    ; page, the guest CPU will read from the correct one and expose the mismatch.
    mov esi, OUTPUT_START
    mov ecx, (OUTPUT_END - OUTPUT_START) + marker_len
.next_byte:
    mov bl, [esi]           ; hardware page walk — correct GVA→GPA
.wait_tx:
    mov dx, 0x3fd           ; Line Status Register
    in al, dx
    test al, 0x20           ; TX holding register empty?
    jz .wait_tx
    mov al, bl
    mov dx, 0x3f8           ; COM1 data register
    out dx, al              ; non-string PIO — bypasses read_memory
    inc esi
    dec ecx
    jnz .next_byte

    ; Delay (~1s at typical QEMU speed)
    mov ecx, 0x3000000
.spin:
    dec ecx
    jnz .spin
    jmp main_loop

marker: db "HELLO VIA OUTSB", 13, 10
marker_len equ $ - marker

; GDT with null, code, and data segments
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

; Pad to 510 bytes and add boot signature
times 510 - ($ - $$) db 0
dw 0xaa55

; Pad to 8KB so QEMU accepts this as a valid disk image
times 8192 - ($ - $$) db 0
