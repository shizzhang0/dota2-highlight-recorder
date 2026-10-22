//! 打印 mp4 文件的时长（读 moov/mvhd）。
//!
//! cargo run -p media --example duration -- <文件>...

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        match media::mp4_duration(path.as_ref()) {
            Ok(secs) => println!("{secs:.3}\t{path}"),
            Err(e) => println!("错误\t{path}\t{e:#}"),
        }
    }
    Ok(())
}
