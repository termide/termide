fn main() {
    // Ensure that tree-sitter C library links correctly
    // Using cc crate to compile tree-sitter runtime

    // Tree-sitter provides its own runtime through the tree-sitter crate
    // Need to ensure that C runtime is compiled correctly

    // For tree-sitter 0.24+ we need to explicitly enable C runtime
    // This is done automatically through cc crate in the dependency tree

    println!("cargo:rerun-if-changed=build.rs");
}
