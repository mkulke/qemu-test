fn main() {
    println!("cargo::rerun-if-changed=payload/guest.bin");
    println!("cargo::rerun-if-changed=payload/guest_pio_str.bin");
    println!("cargo::rerun-if-changed=payload/guest_pio_vmport.bin");
    println!("cargo::rerun-if-changed=payload/guest_avx2.bin");
    println!("cargo::rerun-if-changed=payload/guest_mmio.bin");
    println!("cargo::rerun-if-changed=payload/guest_mmio_regs.bin");
}
