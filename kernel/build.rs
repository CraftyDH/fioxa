fn main() {
    // Magic thing that makes cargo link against our custom linker script
    // Probably a better way to do this
    println!("cargo:rustc-link-arg=link.ld")
}
