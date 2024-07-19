fn main() {
    cxx_build::bridge("src/lib.rs")
        .file("src/extractor.cc")
        .flag("/DYNAMICBASE Cabinet.lib")
        .std("c++14")
        .compile("extractor");


    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/extractor.cc");
    println!("cargo:rerun-if-changed=include/extractor.h");
}