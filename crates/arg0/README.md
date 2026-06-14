# devo-arg0

`devo-arg0` provides the startup wrapper that lets `devo` and `devo-server`
share a single executable. It dispatches based on the process name: on Unix,
`devo-server` can be a symlink to the main binary, while on Windows a generated
batch wrapper passes the intended alias through a sentinel argument.

The crate also performs early process setup before the main CLI runs. It loads
the user `.env` file with protected `DEVO_` variables filtered out, creates
temporary alias entries and prepends them to `PATH`, cleans up stale alias
directories, and starts the Tokio runtime used by the application.

Typical use is to wrap the main entry point with `devo_arg0::run_as`:

```rust
fn main() -> anyhow::Result<()> {
    devo_arg0::run_as(|paths| async move {
        // normal CLI logic
        Ok(())
    })
}
```
