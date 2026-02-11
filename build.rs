fn main() {
    let mut build = cc::Build::new();
    build
        .opt_level(3)
        .warnings(false)
        .define("NDEBUG", None)
        .flag_if_supported("-fsigned-char")
        .include("vendor/cyth/third_party/mir")
        .include("vendor/cyth/third_party/bdwgc/include")
        .file("vendor/cyth/src/checker.c")
        .file("vendor/cyth/src/environment.c")
        .file("vendor/cyth/src/lexer.c")
        .file("vendor/cyth/src/jit.c")
        .file("vendor/cyth/src/map.c")
        .file("vendor/cyth/src/memory.c")
        .file("vendor/cyth/src/parser.c")
        .file("vendor/cyth/third_party/mir/mir.c")
        .file("vendor/cyth/third_party/mir/mir-gen.c")
        .file("vendor/cyth/third_party/bdwgc/extra/gc.c");

    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "x86_64") {
            build.file("vendor/cyth/src/longjmp.x64.asm");
        } else {
            build.file("vendor/cyth/src/longjmp.arm64.asm");
        }
    }

    build.compile("cyth");
}
