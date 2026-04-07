fn main() {
    println!("cargo::rerun-if-changed=payload/guest.bin");
    println!("cargo::rerun-if-changed=payload/guest_pio_str.bin");
}
