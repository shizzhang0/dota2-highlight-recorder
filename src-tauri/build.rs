use std::path::Path;

/// 前端在编译期嵌进程序（tauri.conf.json 的 frontendDist），cargo 默认不把 ui/ 当依赖，
/// 只改前端时不会重编。逐个文件声明（目录形式 cargo 认不出，会让每次都重编），做法同 dota2-game-helper2
fn watch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            watch(&p);
        } else {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
}

fn main() {
    watch(Path::new("../ui"));
    // 发过 rerun-if-changed 之后 cargo 只认声明过的文件，图标也要声明，不然换了图标 exe 里的不会变
    watch(Path::new("icons"));
    // Windows 主线程默认栈 1MB，调试版一启动就栈溢出（2026-10-26 实测），调到 8MB
    println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    tauri_build::build()
}
