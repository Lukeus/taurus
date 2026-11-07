//! Prints one model's vectors for a few texts, for comparing against a
//! reference implementation. `cargo run --release -p taurus-embed --example
//! parity -- <model-dir> <model-id> "text" ...` prints JSON.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let spec = taurus_embed::spec(&args[2]).expect("a known model id");
    let embedder = taurus_embed::Embedder::load(std::path::Path::new(&args[1]), spec).unwrap();
    let vectors = embedder.embed(&args[3..]).unwrap();
    println!("{}", serde_json::to_string(&vectors).unwrap());
}
