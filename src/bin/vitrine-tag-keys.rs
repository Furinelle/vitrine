//! Prints `key<TAB>stored form<TAB>tag` for each tag read from stdin (one per line), using the
//! Worker's own `tag_key`, so offline merge plans match what ingest enforces.
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let tag = line?;
        writeln!(
            out,
            "{}\t{}\t{}",
            vitrine::tag_key(&tag),
            vitrine::normalize_tag(&tag),
            tag
        )?;
    }
    Ok(())
}
