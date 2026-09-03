mod cli;

/// Test-only: keeps the base-help coach, and CHANGELOG.md, in step with this binary.
#[cfg(test)]
mod help_docs;

fn main() {
    cli::run();
}
