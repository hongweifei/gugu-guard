fn main() {
    // web/ 由 rust-embed 在编译期读入，cargo 默认不跟踪这些文件；
    // 不声明依赖的话，改完前端再 `cargo build` 会沿用旧的嵌入资源。
    println!("cargo:rerun-if-changed=../../web");
}
