mod cli;

/// Test-only: keeps the base-help coach, and CHANGELOG.md, in step with this binary.
#[cfg(test)]
mod help_docs;

/// Startup stack for the thread that runs the CLI (#129).
///
/// **Why this exists at all.** `clap`'s derive-generated `Command` tree is built in full
/// *before* any argument is dispatched, so the cost is paid by `--version` as much as by
/// real work. At `opt-level = 0` none of those builder frames are inlined, and the 24
/// `#[derive(Subcommand)]` enums in `cli.rs` need **more startup stack than Windows gives
/// the main thread by default** — every invocation of a debug build aborted with
/// `STATUS_STACK_OVERFLOW` before reaching a line of product code.
///
/// **Measured at `2e5d2a4a`, debug profile, one binary with only the size varied** (so the
/// parameter is demonstrably in effect, not merely present here): **1152 KB still aborts,
/// 1168 KB runs clean**, bisected to 16 KB. The requirement is ~1.14 MB against a Windows
/// default of 1 MB — it fails by about 14%, which is why it looked profile-specific rather
/// than structural.
///
/// **Why 8 MB and not the ~1.2 MB that would just fit.** 8 MB is exactly the Linux
/// main-thread default (`ulimit -s` = 8192 KB), and Linux is the platform where this never
/// reproduced — all four CI jobs are `ubuntu-latest`, which is precisely why CI stayed
/// green while every Windows debug run died. This gives Windows parity with the platform
/// base is already tested on, plus ~7x the measured requirement, so adding a 25th
/// subcommand cannot quietly re-break it. Sizing it to the measurement instead would put
/// the margin back at 14%.
///
/// This states the requirement rather than masking it: `[profile.dev] opt-level = 1` also
/// stops the abort (measured), but only by shrinking the frames until they fit, which
/// leaves release depending on inlining happening to fit under 1 MB.
const STARTUP_STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() {
    let worker = std::thread::Builder::new()
        .name("base-main".to_string())
        .stack_size(STARTUP_STACK_BYTES)
        .spawn(cli::run)
        .expect("spawning the base worker thread");

    // `cli::run` signals failure with `std::process::exit`, which terminates the whole
    // process from any thread, so ordinary exit codes still propagate untouched.
    //
    // A PANIC does not: it unwinds the worker and leaves `join` returning `Err`, and a
    // `main` that ignored that would return normally and **exit 0**. Every failing command
    // would then report success — the same "could not run is indistinguishable from
    // passed" defect that #129 itself creates, reintroduced by the fix for it. 101 is the
    // code rustc's own harness uses for a panicking process.
    if worker.join().is_err() {
        std::process::exit(101);
    }
}
