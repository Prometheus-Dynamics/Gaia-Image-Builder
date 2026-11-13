fn main() {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "aarch64")] {
            let arch = "aarch64";
        } else {
            let arch = "another architecture";
        }
    }
    println!("hello from Rust on {arch}");
}
