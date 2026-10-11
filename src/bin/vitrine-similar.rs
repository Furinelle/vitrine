use std::io::{self, Read};
use vitrine::similar::{compute_offline, SimilarInput, SIMILAR_ALGORITHM};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("{SIMILAR_ALGORITHM}");
        return;
    }
    if let Err(error) = run() {
        eprintln!("similarity computation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    const MAX_INPUT: u64 = 128 * 1024 * 1024;
    let mut input = Vec::new();
    io::stdin().take(MAX_INPUT + 1).read_to_end(&mut input)?;
    if input.len() as u64 > MAX_INPUT {
        return Err("snapshot exceeds 128 MiB".into());
    }
    let snapshot: SimilarInput = serde_json::from_slice(&input)?;
    drop(input);
    let result = compute_offline(snapshot)?;
    serde_json::to_writer(io::stdout().lock(), &result)?;
    Ok(())
}
