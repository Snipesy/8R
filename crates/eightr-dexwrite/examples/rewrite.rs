//! `cargo run -p eightr-dexwrite --example rewrite -- in.dex out.dex`: read and re-write a dex.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("read input");
    let dex = eightr_dex::Dex::parse(&bytes).expect("parse");
    let program = eightr_ir::model::Program::load(&[&dex]).expect("load");
    std::fs::write(&args[2], eightr_dexwrite::write_all(&program).expect("write")).expect("write output");
}
