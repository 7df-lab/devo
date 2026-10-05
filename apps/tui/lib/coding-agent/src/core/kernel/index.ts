/**
 * Kernel type surface for the TUI. Execution is Rust Native
 * (`crates/kernel` spawns `python -m rlm.repl`); nothing spawns kernels here.
 */
export * from "./shared.js";
