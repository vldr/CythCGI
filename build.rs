fn main() {
    println!("cargo:rustc-link-search=native=vendor/cyth");
    println!("cargo:rustc-link-lib=static=cyth");
}
