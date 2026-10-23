# whisper-rs-sys, as Taurus carries it

This is [whisper-rs-sys](https://crates.io/crates/whisper-rs-sys) 0.14.1, the
FFI crate under `whisper-rs` 0.15.1, with whisper.cpp's sources inside it. The
workspace's `Cargo.toml` swaps it in with `[patch.crates-io]`. It's excluded
from the workspace, so `cargo clippy --workspace` and `cargo test --workspace`
don't run over upstream code.

Licenses: the crate is under the Unlicense, whisper.cpp under MIT
(`whisper.cpp/LICENSE`).

## What's changed

Only `build.rs`, and only where the Rust declarations for whisper.cpp come from.
Look for `write_bindings`.

Upstream runs bindgen. When that fails, it quietly copies `src/bindings.rs`,
which was generated on Linux against glibc. On Windows those bindings fail
three layout assertions at compile time:

```
error[E0080]: attempt to compute `12_usize - 16_usize`, which would overflow
```

They'd be wrong on Windows even if they compiled, because C's `long` is 32 bits
there. And bindgen on Windows needs a libclang that finds both its own headers
and MSVC's, which is the step most likely to fail on someone's machine. A
correctly installed LLVM still failed with `'stdbool.h' file not found`.

So here:

- **A target with a shipped `src/bindings-<target>.rs` uses it, and bindgen
  never runs.** That's `x86_64-pc-windows-msvc`. A Windows build needs CMake
  and Visual Studio's C++ tools, and no LLVM.
- **Every other target runs bindgen.** A failure stops the build and says what
  to install. Nothing falls back, and the Linux `src/bindings.rs` is removed,
  so there's nothing to fall back to. `WHISPER_DONT_GENERATE_BINDINGS` no
  longer does anything.

## Keeping the Windows bindings right

CI's **Whisper bindings (Windows)** job builds this crate with
`TAURUS_WHISPER_BINDINGS` set to a folder. In that mode bindgen always runs,
the result is written to the folder, and the build fails if it differs from
the shipped file. The folder is uploaded as the `whisper-bindings-windows`
artifact either way.

When that job fails, download the artifact and copy
`bindings-x86_64-pc-windows-msvc.rs` over `src/`. To do the same on a Windows
machine with LLVM installed:

```powershell
$env:TAURUS_WHISPER_BINDINGS = "$PWD\target\whisper-bindings"
cargo build -p whisper-rs-sys
```

## Upgrading

1. Replace everything here except this file and `src/bindings-*.rs` with the
   new release from crates.io, and bump `whisper-rs` in
   `crates/taurus-voice/Cargo.toml` to match.
2. Put `write_bindings` and `generate` back into `build.rs`. Delete the new
   `src/bindings.rs`.
3. Push. The Windows job fails because the headers changed. Take its artifact
   as above.
4. Check `rust-version`. The workspace promises 1.87, and whisper-rs 0.16
   already needs 1.88 (`slice::as_chunks`).
