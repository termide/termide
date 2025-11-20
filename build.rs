fn main() {
    // Убедиться, что tree-sitter C библиотека линкуется правильно
    // Используем cc crate для компиляции runtime tree-sitter

    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Tree-sitter предоставляет собственный runtime через крейт tree-sitter
    // Нужно убедиться что C runtime правильно скомпилирован

    // Для tree-sitter 0.24+ нужно явно включить C runtime
    // Это делается автоматически через cc crate в дереве зависимостей

    println!("cargo:rerun-if-changed=build.rs");
}
