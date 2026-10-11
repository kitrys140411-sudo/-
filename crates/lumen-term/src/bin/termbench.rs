//! Throughput benchmark of the emulation core (parser + grid), on synthetic
//! workloads that resemble real terminal output.
//!
//!   lumen-termbench [MEGABYTES]

use std::time::Instant;

use lumen_term::term::Terminal;

fn workload(name: &str, mb: usize) -> Vec<u8> {
    let target = mb * 1024 * 1024;
    let mut out = Vec::with_capacity(target + 4096);
    let mut i = 0u64;
    while out.len() < target {
        match name {
            "ascii" => {
                out.extend_from_slice(
                    format!("{i:08} The quick brown fox jumps over the lazy dog; lorem ipsum dolor sit amet, consectetur.\r\n").as_bytes(),
                );
            }
            "color" => {
                // Compiler/ls-like output: short colored runs.
                out.extend_from_slice(
                    format!(
                        "\x1b[1;32m{i:6}\x1b[0m \x1b[34msrc/module_{}.rs\x1b[0m:\x1b[33m{}\x1b[0m: \x1b[1;31merror\x1b[0m[E{:04}]: \x1b[38;5;{}mmismatched\x1b[0m \x1b[38;2;200;120;{}mtypes\x1b[0m\r\n",
                        i % 97,
                        i % 1000,
                        i % 9999,
                        i % 256,
                        i % 256
                    )
                    .as_bytes(),
                );
            }
            "unicode" => {
                out.extend_from_slice(format!("{i:06} Ünïcödé — 日本語のテキスト 한국어 😀🎉 e\u{301}a\u{308} ✓ ╭──╮ │ok│ ╰──╯\r\n").as_bytes());
            }
            "scroll" => {
                // Full-screen app redrawing a region (vim-like scrolling).
                out.extend_from_slice(format!("\x1b[2;23r\x1b[23;1H\n\x1b[23;1H\x1b[K{i:08} redraw line with \x1b[7mreverse\x1b[0m text\x1b[r").as_bytes());
            }
            _ => unreachable!(),
        }
        i += 1;
    }
    out
}

fn main() {
    let mb: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(32);
    println!("lumen-term emulation throughput ({mb} MiB per workload, 120x40, 10000 lines history)");
    for name in ["ascii", "color", "unicode", "scroll"] {
        let data = workload(name, mb);
        let mut t = Terminal::new(120, 40, 10_000);
        let start = Instant::now();
        // Feed in chunks like PTY reads.
        for chunk in data.chunks(65536) {
            t.advance(chunk);
        }
        let secs = start.elapsed().as_secs_f64();
        let rate = data.len() as f64 / secs / (1024.0 * 1024.0);
        println!("  {name:8} {rate:8.1} MiB/s  ({:.3} s)", secs);
        std::hint::black_box(&t.term.row_text(0));
    }
}
