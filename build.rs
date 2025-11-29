fn main() {
    cc::Build::new()
        .include("vendor/cyth/third_party/mir")
        .file("vendor/cyth/src/checker.c")
        .file("vendor/cyth/src/environment.c")
        .file("vendor/cyth/src/lexer.c")
        .file("vendor/cyth/src/main.c")
        .file("vendor/cyth/src/jit.c")
        .file("vendor/cyth/src/map.c")
        .file("vendor/cyth/src/memory.c")
        .file("vendor/cyth/src/parser.c")
        .file("vendor/cyth/third_party/mir/mir.c")
        .file("vendor/cyth/third_party/mir/mir-gen.c")
        .warnings(false)
        .define("NDEBUG", None)
        .opt_level(4)
        .compile("cyth");
}
